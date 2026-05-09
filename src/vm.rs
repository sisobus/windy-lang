//! Windy bytecode VM — multi-IP execution loop (SPEC §3.5, §3.6, §4).
//!
//! The VM holds an **ordered list of IP contexts** plus the shared grid.
//! One `step()` advances every live IP once, in birth order, then promotes
//! any newly spawned IPs from `pending_spawns` and removes IPs marked
//! `halted` by `@` during the tick. `max_steps` counts ticks, not IP
//! advances — this matches SPEC §3.6.
//!
//! Stack underflow yields 0 (§3.3, §7). Division and modulo by zero push
//! 0 (§7). `GRID_PUT` writes re-take effect on the next visit because
//! decoding is on-demand.
//!
//! Streams are passed as `&mut dyn` parameters to `step` / `run` rather
//! than stored on the Vm. This lets the debugger inspect its captured
//! stdout between ticks without fighting the borrow checker.

use crate::easter::{banner, detect};
use crate::grid::{Grid, Ip, SPACE};
use crate::opcodes::{decode_cell, Op};
use crate::parser::parse;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, ToPrimitive, Zero};
use rand::seq::SliceRandom;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

const WINDS: [(i64, i64); 8] = [
    (1, 0), (1, -1), (0, -1), (-1, -1),
    (-1, 0), (-1, 1), (0, 1), (1, 1),
];

const EAST: (i64, i64) = (1, 0);
const NORTH: (i64, i64) = (0, -1);
const WEST: (i64, i64) = (-1, 0);
const SOUTH: (i64, i64) = (0, 1);
const NE: (i64, i64) = (1, -1);
const NW: (i64, i64) = (-1, -1);
const SW: (i64, i64) = (-1, 1);
const SE: (i64, i64) = (1, 1);

const STR_QUOTE: u32 = 0x22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    Ok,
    MaxSteps,
    /// Runtime trap (e.g. CALM at speed 1). Per SPEC §3.7, traps abort
    /// the program with a non-zero exit code; we use 134 (matches the
    /// POSIX SIGABRT convention) so it's distinguishable from `--max-
    /// steps`'s 124 and from a normal halt.
    Trap,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        match self {
            ExitCode::Ok => 0,
            ExitCode::MaxSteps => 124,
            ExitCode::Trap => 134,
        }
    }
}

/// Top-level runtime configuration used by `run_source`. Stream refs are
/// borrowed so callers retain ownership of the underlying buffers.
pub struct RunOptions<'a> {
    pub seed: Option<u64>,
    pub max_steps: Option<u64>,
    pub stdin: &'a mut dyn Read,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

/// Per-IP state. The grid is shared across the whole `Vm`; every other
/// bit of VM state is per-IP and lives here.
#[derive(Debug, Clone)]
pub struct IpContext {
    pub ip: Ip,
    pub stack: Vec<BigInt>,
    pub strmode: bool,
    /// Marked by `@`; the main loop removes halted IPs at the end of
    /// the tick so other IPs can still observe this IP's position
    /// during the same tick. The collision pass (SPEC §3.8) also uses
    /// this to mark absorbed and head-on-cancelled IPs for end-of-tick
    /// removal.
    pub halted: bool,
    /// Wind speed — strictly positive (SPEC §3.7). Initial value is
    /// `1`. Carried as `BigInt` to match the language's
    /// unbounded-arithmetic promise (SPEC §2 #4).
    pub speed: BigInt,
}

impl IpContext {
    fn new_root() -> Self {
        Self {
            ip: Ip::default(),
            stack: Vec::new(),
            strmode: false,
            halted: false,
            speed: BigInt::one(),
        }
    }
}

pub struct Vm {
    pub grid: Grid,
    /// Ordered live-IP list (birth order, oldest first).
    pub ips: Vec<IpContext>,
    /// IPs spawned during the current tick. Promoted into `ips` at the
    /// end of the tick so a freshly spawned IP does not run twice.
    pending_spawns: Vec<IpContext>,
    /// True once `ips` has been emptied. Latched for the benefit of
    /// callers that inspect `Vm` between steps.
    pub halted: bool,
    /// Number of ticks executed so far (one tick = one pass over the
    /// live IP list — SPEC §3.6).
    pub steps: u64,
    pub max_steps: Option<u64>,
    /// Set when a runtime trap occurs (currently only CALM-at-speed-1,
    /// SPEC §3.7). The main loop returns `ExitCode::Trap` after the
    /// current tick completes.
    pub trapped: bool,
    rng: ChaCha8Rng,
    warned: HashSet<u32>,
    /// Optional execution counters used by downstream tooling (e.g.
    /// windy-coin's mining-policy guest). Kept in lockstep with the
    /// opcode dispatch so a feature consumer can read a snapshot at
    /// any point without instrumenting the interpreter themselves.
    /// Only present when the `metrics` feature is enabled — disabled
    /// builds incur no cost.
    #[cfg(feature = "metrics")]
    pub metrics: VmMetrics,
}

/// Per-run execution metrics surfaced for downstream policy code.
///
/// Counts are *cumulative*, not per-tick: `spawned_ips` counts every
/// `t` invocation across the run, not just the latest tick's. The
/// bitmap follows the convention of bit `i` being set ↔ at least one
/// invocation of the corresponding opcode happened. See the `BIT_*`
/// associated constants for the bit assignment.
///
/// `max_alive_ips` tracks the peak measured at the *end* of each tick
/// — i.e. after collision merges and post-halt retainment — so it's
/// the steady-state count a policy contract would care about, not the
/// transient mid-tick split count.
#[cfg(feature = "metrics")]
#[derive(Debug, Default, Clone, Copy)]
pub struct VmMetrics {
    /// Peak `vm.ips.len()` observed at the end of a tick over the run.
    /// Initialised to `1` (the root IP) when the VM is constructed.
    pub max_alive_ips: u64,
    /// Total `t` (SPLIT) invocations across all IPs.
    pub spawned_ips: u64,
    /// Total `p` (GRID_PUT) invocations.
    pub grid_writes: u64,
    /// Total `_` + `|` + `~` invocations — anything that picks the
    /// next direction dynamically.
    pub branch_count: u64,
    /// Bitmap of which "hard" opcodes were used at least once. The
    /// hard pool matches the ten opcodes `random.ts` deliberately
    /// excludes — see windy-coin's Phase 2 mining policy for the
    /// rationale. Bits 10..15 are reserved.
    pub hard_opcode_bitmap: u16,
}

#[cfg(feature = "metrics")]
impl VmMetrics {
    pub const BIT_T:           u16 = 1 << 0;  // SPLIT
    pub const BIT_P:           u16 = 1 << 1;  // GRID_PUT
    pub const BIT_G:           u16 = 1 << 2;  // GRID_GET
    pub const BIT_IFH:         u16 = 1 << 3;  // _
    pub const BIT_IFV:         u16 = 1 << 4;  // |
    pub const BIT_GUST:        u16 = 1 << 5;  // ≫
    pub const BIT_CALM:        u16 = 1 << 6;  // ≪
    pub const BIT_TURBULENCE:  u16 = 1 << 7;  // ~
    pub const BIT_TRAMPOLINE:  u16 = 1 << 8;  // #
    pub const BIT_STRMODE:     u16 = 1 << 9;  // "
}

impl Vm {
    pub fn new(grid: Grid, seed: Option<u64>, max_steps: Option<u64>) -> Self {
        let rng = match seed {
            Some(s) => ChaCha8Rng::seed_from_u64(s),
            None => ChaCha8Rng::from_entropy(),
        };
        Self {
            grid,
            ips: vec![IpContext::new_root()],
            pending_spawns: Vec::new(),
            halted: false,
            steps: 0,
            max_steps,
            trapped: false,
            rng,
            warned: HashSet::new(),
            #[cfg(feature = "metrics")]
            metrics: VmMetrics {
                // Root IP is alive from construction, so the floor for
                // peak concurrent-IPs is 1 — never 0.
                max_alive_ips: 1,
                spawned_ips: 0,
                grid_writes: 0,
                branch_count: 0,
                hard_opcode_bitmap: 0,
            },
        }
    }

    /// Convenience accessor for callers (debugger, wasm API) that want
    /// to report "the IP" for single-IP programs. Returns the oldest
    /// live IP, or `None` when the program has halted entirely.
    pub fn first_ip(&self) -> Option<&IpContext> {
        self.ips.first()
    }

    pub fn run(
        &mut self,
        stdin: &mut dyn Read,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) -> ExitCode {
        while !self.halted {
            if let Some(cap) = self.max_steps {
                if self.steps >= cap {
                    return ExitCode::MaxSteps;
                }
            }
            self.step(stdin, stdout, stderr);
            self.steps += 1;
            if self.trapped {
                return ExitCode::Trap;
            }
        }
        ExitCode::Ok
    }

    /// Execute one full tick: every live IP takes one step, in birth
    /// order. IPs spawned via `t` during this tick wait until the next
    /// tick; IPs that executed `@` are removed at the end of this tick.
    /// After movement, the runtime runs a collision pass that merges
    /// any IPs that share a cell (SPEC §3.8).
    pub fn step(
        &mut self,
        stdin: &mut dyn Read,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) {
        let n = self.ips.len();
        for i in 0..n {
            if self.ips[i].halted {
                continue;
            }
            let cell = self.grid.get(self.ips[i].ip.x, self.ips[i].ip.y);
            if self.ips[i].strmode {
                if cell.to_u32() == Some(STR_QUOTE) {
                    self.ips[i].strmode = false;
                } else {
                    self.ips[i].stack.push(cell.clone());
                }
            } else {
                let (op, operand) = decode_cell(&cell);
                self.execute(i, op, operand, &cell, stdin, stdout, stderr);
            }
            if !self.ips[i].halted {
                self.advance_with_speed(i);
            }
        }
        // End of tick: promote spawns, collide, then drop halted.
        if !self.pending_spawns.is_empty() {
            self.ips.extend(self.pending_spawns.drain(..));
        }
        self.collision_pass();
        self.ips.retain(|c| !c.halted);
        if self.ips.is_empty() {
            self.halted = true;
        }

        // Steady-state IP count for this tick — measured *after* the
        // collision pass and the halted-IP retainment. Mid-tick peaks
        // (e.g. right after a `t` SPLIT, before the next collision
        // merges them) aren't visible here on purpose: the peak we
        // care about is the one a downstream policy could see across
        // tick boundaries.
        #[cfg(feature = "metrics")]
        {
            let alive = self.ips.len() as u64;
            if alive > self.metrics.max_alive_ips {
                self.metrics.max_alive_ips = alive;
            }
        }
    }

    /// Movement (SPEC §3.7): `pos += dir × speed`, only the
    /// destination cell will execute on the next tick (intermediate
    /// cells skipped). Speed is BigInt; we clamp to `i64::MAX` for the
    /// i64 coordinate arithmetic — consistent with the pragmatic i64
    /// coords noted in `grid.rs`. A speed beyond i64::MAX simply
    /// launches the IP into the empty far field where every cell is
    /// space (NOP).
    fn advance_with_speed(&mut self, i: usize) {
        let speed_i64 = self.ips[i].speed.to_i64().unwrap_or(i64::MAX);
        let dx = self.ips[i].ip.dx.saturating_mul(speed_i64);
        let dy = self.ips[i].ip.dy.saturating_mul(speed_i64);
        self.ips[i].ip.x = self.ips[i].ip.x.saturating_add(dx);
        self.ips[i].ip.y = self.ips[i].ip.y.saturating_add(dy);
    }

    /// Collision pass (SPEC §3.8): group live IPs by `(x, y)` and
    /// merge each non-singleton group in birth order. Stack
    /// concatenation keeps the oldest IP's stack at the bottom;
    /// direction is the per-axis sum clipped to `{-1, 0, +1}` (sum of
    /// `(0, 0)` ⇒ merged IP dies); speed is the max of the
    /// constituents; strmode resets to off. Absorbed IPs are marked
    /// halted so the post-collision `retain` drops them.
    fn collision_pass(&mut self) {
        let mut groups: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (i, ctx) in self.ips.iter().enumerate() {
            if ctx.halted {
                continue;
            }
            groups
                .entry((ctx.ip.x, ctx.ip.y))
                .or_default()
                .push(i);
        }

        for (_, indices) in groups {
            if indices.len() < 2 {
                continue;
            }
            // `indices` is in birth order because we iterated self.ips
            // sequentially. The oldest IP is the survivor; later IPs
            // are absorbed.
            let survivor = indices[0];
            let mut summed_dx: i64 = self.ips[survivor].ip.dx;
            let mut summed_dy: i64 = self.ips[survivor].ip.dy;
            let mut max_speed: BigInt = self.ips[survivor].speed.clone();
            for &absorbed in &indices[1..] {
                let stk = std::mem::take(&mut self.ips[absorbed].stack);
                self.ips[survivor].stack.extend(stk);
                summed_dx += self.ips[absorbed].ip.dx;
                summed_dy += self.ips[absorbed].ip.dy;
                if self.ips[absorbed].speed > max_speed {
                    max_speed = self.ips[absorbed].speed.clone();
                }
                self.ips[absorbed].halted = true;
            }
            let cdx = summed_dx.signum();
            let cdy = summed_dy.signum();
            if cdx == 0 && cdy == 0 {
                // Head-on storm: merged IP dies; absorbed stacks
                // drop with it.
                self.ips[survivor].halted = true;
            } else {
                self.ips[survivor].ip.dx = cdx;
                self.ips[survivor].ip.dy = cdy;
                self.ips[survivor].speed = max_speed;
                self.ips[survivor].strmode = false;
            }
        }
    }

    fn pop_at(&mut self, i: usize) -> BigInt {
        self.ips[i].stack.pop().unwrap_or_else(BigInt::zero)
    }

    fn push_at(&mut self, i: usize, v: BigInt) {
        self.ips[i].stack.push(v);
    }

    fn set_dir(&mut self, i: usize, d: (i64, i64)) {
        self.ips[i].ip.set_dir(d.0, d.1);
    }

    fn execute(
        &mut self,
        i: usize,
        op: Op,
        operand: u32,
        cell: &BigInt,
        stdin: &mut dyn Read,
        stdout: &mut dyn Write,
        stderr: &mut dyn Write,
    ) {
        match op {
            Op::Nop => {}
            Op::Halt => self.ips[i].halted = true,
            Op::Trampoline => {
                #[cfg(feature = "metrics")]
                { self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_TRAMPOLINE; }
                self.ips[i].ip.advance();
            }
            Op::Split => {
                #[cfg(feature = "metrics")]
                {
                    self.metrics.spawned_ips = self.metrics.spawned_ips.saturating_add(1);
                    self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_T;
                }
                let here = self.ips[i].ip;
                // SPEC §3.5 / §3.7: child inherits parent's speed at
                // split time.
                let parent_speed = self.ips[i].speed.clone();
                let new_ctx = IpContext {
                    ip: Ip {
                        x: here.x - here.dx,
                        y: here.y - here.dy,
                        dx: -here.dx,
                        dy: -here.dy,
                    },
                    stack: Vec::new(),
                    strmode: false,
                    halted: false,
                    speed: parent_speed,
                };
                self.pending_spawns.push(new_ctx);
            }
            Op::MoveE => self.set_dir(i, EAST),
            Op::MoveNe => self.set_dir(i, NE),
            Op::MoveN => self.set_dir(i, NORTH),
            Op::MoveNw => self.set_dir(i, NW),
            Op::MoveW => self.set_dir(i, WEST),
            Op::MoveSw => self.set_dir(i, SW),
            Op::MoveS => self.set_dir(i, SOUTH),
            Op::MoveSe => self.set_dir(i, SE),
            Op::Turbulence => {
                #[cfg(feature = "metrics")]
                {
                    self.metrics.branch_count = self.metrics.branch_count.saturating_add(1);
                    self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_TURBULENCE;
                }
                let d = *WINDS.choose(&mut self.rng).unwrap();
                self.set_dir(i, d);
            }
            Op::PushDigit => self.push_at(i, BigInt::from(operand)),
            Op::StrMode => {
                #[cfg(feature = "metrics")]
                { self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_STRMODE; }
                self.ips[i].strmode = true;
            }
            Op::Add => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                self.push_at(i, a + b);
            }
            Op::Sub => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                self.push_at(i, a - b);
            }
            Op::Mul => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                self.push_at(i, a * b);
            }
            Op::Div => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                let r = if b.is_zero() { BigInt::zero() } else { a.div_floor(&b) };
                self.push_at(i, r);
            }
            Op::Mod => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                let r = if b.is_zero() { BigInt::zero() } else { a.mod_floor(&b) };
                self.push_at(i, r);
            }
            Op::Not => {
                let a = self.pop_at(i);
                self.push_at(i, if a.is_zero() { BigInt::one() } else { BigInt::zero() });
            }
            Op::Gt => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                self.push_at(i, if a > b { BigInt::one() } else { BigInt::zero() });
            }
            Op::Dup => {
                let top = self.pop_at(i);
                self.push_at(i, top.clone());
                self.push_at(i, top);
            }
            Op::Drop => {
                let _ = self.pop_at(i);
            }
            Op::Swap => {
                let b = self.pop_at(i);
                let a = self.pop_at(i);
                self.push_at(i, b);
                self.push_at(i, a);
            }
            Op::IfH => {
                #[cfg(feature = "metrics")]
                {
                    self.metrics.branch_count = self.metrics.branch_count.saturating_add(1);
                    self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_IFH;
                }
                let a = self.pop_at(i);
                self.set_dir(i, if a.is_zero() { EAST } else { WEST });
            }
            Op::IfV => {
                #[cfg(feature = "metrics")]
                {
                    self.metrics.branch_count = self.metrics.branch_count.saturating_add(1);
                    self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_IFV;
                }
                let a = self.pop_at(i);
                self.set_dir(i, if a.is_zero() { SOUTH } else { NORTH });
            }
            Op::PutNum => {
                let a = self.pop_at(i);
                let _ = write!(stdout, "{} ", a);
            }
            Op::PutChr => {
                let a = self.pop_at(i);
                if let Some(cp) = a.to_u32() {
                    if let Some(c) = char::from_u32(cp) {
                        let _ = write!(stdout, "{}", c);
                    }
                }
            }
            Op::GetNum => {
                let v = read_num_input(stdin).unwrap_or_else(|| BigInt::from(-1));
                self.push_at(i, v);
            }
            Op::GetChr => {
                let v = match read_utf8_char(stdin) {
                    Ok(Some(c)) => BigInt::from(c as u32),
                    _ => BigInt::from(-1),
                };
                self.push_at(i, v);
            }
            Op::GridGet => {
                #[cfg(feature = "metrics")]
                { self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_G; }
                let y = self.pop_at(i);
                let x = self.pop_at(i);
                let (xi, yi) = match (x.to_i64(), y.to_i64()) {
                    (Some(xi), Some(yi)) => (xi, yi),
                    _ => {
                        self.push_at(i, BigInt::from(SPACE));
                        return;
                    }
                };
                let v = self.grid.get(xi, yi);
                self.push_at(i, v);
            }
            Op::GridPut => {
                #[cfg(feature = "metrics")]
                {
                    self.metrics.grid_writes = self.metrics.grid_writes.saturating_add(1);
                    self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_P;
                }
                let y = self.pop_at(i);
                let x = self.pop_at(i);
                let v = self.pop_at(i);
                if let (Some(xi), Some(yi)) = (x.to_i64(), y.to_i64()) {
                    self.grid.put(xi, yi, v);
                }
            }
            Op::Gust => {
                #[cfg(feature = "metrics")]
                { self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_GUST; }
                self.ips[i].speed += BigInt::one();
            }
            Op::Calm => {
                #[cfg(feature = "metrics")]
                { self.metrics.hard_opcode_bitmap |= VmMetrics::BIT_CALM; }
                if self.ips[i].speed <= BigInt::one() {
                    let (x, y) = (self.ips[i].ip.x, self.ips[i].ip.y);
                    let _ = writeln!(
                        stderr,
                        "windy: trap: calm in still air (CALM at speed 1, IP #{} at ({}, {}))",
                        i, x, y
                    );
                    self.ips[i].halted = true;
                    self.trapped = true;
                    return;
                }
                self.ips[i].speed -= BigInt::one();
            }
            Op::Unknown => self.warn_unknown(cell, stderr),
        }
    }

    fn warn_unknown(&mut self, cell: &BigInt, stderr: &mut dyn Write) {
        let cp = match cell.to_u32() {
            Some(v) => v,
            None => return,
        };
        if !self.warned.insert(cp) {
            return;
        }
        let ch = char::from_u32(cp).unwrap_or('?');
        let _ = writeln!(
            stderr,
            "windy: warning: unknown glyph {:?} (U+{:04X}) treated as NOP",
            ch, cp
        );
    }
}

/// Parse, print the watermark banner if applicable, then run.
pub fn run_source(source: &str, opts: RunOptions) -> ExitCode {
    let (grid, scan_text) = parse(source);
    if detect(&scan_text) {
        let _ = writeln!(opts.stderr, "{}", banner());
    }
    let mut vm = Vm::new(grid, opts.seed, opts.max_steps);
    vm.run(opts.stdin, opts.stdout, opts.stderr)
}

fn read_num_input(stdin: &mut dyn Read) -> Option<BigInt> {
    let first = loop {
        match read_utf8_char(stdin).ok()? {
            None => return None,
            Some(c) if c.is_whitespace() => continue,
            Some(c) => break c,
        }
    };
    let mut s = String::new();
    s.push(first);
    loop {
        match read_utf8_char(stdin).ok()? {
            None => break,
            Some(c) if c.is_whitespace() => break,
            Some(c) => s.push(c),
        }
    }
    s.parse::<BigInt>().ok()
}

/// Read one UTF-8 char from `reader`. Returns `Ok(None)` on EOF.
pub(crate) fn read_utf8_char(reader: &mut dyn Read) -> std::io::Result<Option<char>> {
    let mut buf = [0u8; 4];
    let n = reader.read(&mut buf[..1])?;
    if n == 0 {
        return Ok(None);
    }
    let first = buf[0];
    let expected = if first < 0x80 {
        1
    } else if first < 0xC0 {
        return Ok(Some(char::REPLACEMENT_CHARACTER));
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    };
    let mut filled = 1;
    while filled < expected {
        match reader.read(&mut buf[filled..expected])? {
            0 => return Ok(Some(char::REPLACEMENT_CHARACTER)),
            k => filled += k,
        }
    }
    Ok(std::str::from_utf8(&buf[..filled])
        .ok()
        .and_then(|s| s.chars().next()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(source: &str) -> (ExitCode, String, String) {
        run_with_stdin(source, b"")
    }

    fn run_with_stdin(source: &str, stdin_bytes: &[u8]) -> (ExitCode, String, String) {
        let mut stdin = stdin_bytes;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_source(
            source,
            RunOptions {
                seed: Some(42),
                max_steps: Some(1_000_000),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        (
            code,
            String::from_utf8(stdout).unwrap(),
            String::from_utf8(stderr).unwrap(),
        )
    }

    #[test]
    fn halt_returns_ok() {
        let (code, out, _) = run("@");
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "");
    }

    #[test]
    fn hello_world() {
        let (code, out, _) = run("\"!dlroW ,olleH\",,,,,,,,,,,,,@");
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "Hello, World!");
    }

    #[test]
    fn put_num_trailing_space() {
        assert_eq!(run("34+.@").1, "7 ");
    }

    #[test]
    fn sub_argument_order() {
        assert_eq!(run("34-.@").1, "-1 ");
    }

    #[test]
    fn div_mod_by_zero_push_zero() {
        assert_eq!(run("50/.@").1, "0 ");
        assert_eq!(run("50%.@").1, "0 ");
    }

    #[test]
    fn stack_underflow_yields_zero() {
        assert_eq!(run(".@").1, "0 ");
    }

    #[test]
    fn gt_comparison() {
        assert_eq!(run("53`.@").1, "1 ");
        assert_eq!(run("35`.@").1, "0 ");
    }

    #[test]
    fn dup_and_swap() {
        assert_eq!(run("7:..@").1, "7 7 ");
        assert_eq!(run("12\\..@").1, "1 2 ");
    }

    #[test]
    fn trampoline_skips_next_cell() {
        assert_eq!(run("#@5.@").1, "5 ");
    }

    #[test]
    fn string_mode_pushes_codepoints() {
        assert_eq!(run("\"A\",@").1, "A");
        assert_eq!(run("\"+\".@").1, "43 ");
    }

    #[test]
    fn if_v_routes_vertically() {
        let src = "0v\n |\n @";
        let (code, out, _) = run(src);
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "");
    }

    #[test]
    fn grid_put_then_get_roundtrip() {
        assert_eq!(run("\"!\"55p55g,@").1, "!");
    }

    #[test]
    fn grid_put_self_modifies_for_halt() {
        let (code, out, err) = run("88*70p X");
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "");
        assert!(!err.contains("unknown glyph"));
    }

    #[test]
    fn max_steps_returns_124() {
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_source(
            "    ",
            RunOptions {
                seed: Some(0),
                max_steps: Some(3),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(code, ExitCode::MaxSteps);
    }

    #[test]
    fn unknown_glyph_warned_once() {
        let (_, _, err) = run("ZZ@");
        assert_eq!(err.matches("unknown glyph").count(), 1);
    }

    #[test]
    fn unknown_glyph_per_codepoint() {
        let (_, _, err) = run("ZY@");
        assert_eq!(err.matches("unknown glyph").count(), 2);
    }

    #[test]
    fn sisobus_banner_on_stderr() {
        let (_, out, err) = run("\"sisobus\"@");
        assert!(err.contains("Kim Sangkeun"));
        assert!(!out.contains("Kim Sangkeun"));
    }

    #[test]
    fn no_banner_without_watermark() {
        let (_, _, err) = run("@");
        assert!(!err.contains("Kim Sangkeun"));
    }

    #[test]
    fn put_chr_emits_unicode() {
        assert_eq!(run("\"가\",@").1, "가");
    }

    #[test]
    fn get_chr_on_empty_stdin_pushes_minus_one() {
        assert_eq!(run_with_stdin("?.@", b"").1, "-1 ");
    }

    #[test]
    fn get_chr_reads_one_char() {
        assert_eq!(run_with_stdin("?.@", b"A").1, "65 ");
    }

    #[test]
    fn get_num_reads_integer() {
        assert_eq!(run_with_stdin("&.@", b"42 ").1, "42 ");
    }

    #[test]
    fn get_num_on_empty_stdin_pushes_minus_one() {
        assert_eq!(run_with_stdin("&.@", b"").1, "-1 ");
    }

    #[test]
    fn turbulence_deterministic_with_seed() {
        let mut s1: &[u8] = b"";
        let mut o1 = Vec::new();
        let mut e1 = Vec::new();
        run_source(
            "~.@\n.@\n.@",
            RunOptions {
                seed: Some(42),
                max_steps: Some(50),
                stdin: &mut s1,
                stdout: &mut o1,
                stderr: &mut e1,
            },
        );
        let mut s2: &[u8] = b"";
        let mut o2 = Vec::new();
        let mut e2 = Vec::new();
        run_source(
            "~.@\n.@\n.@",
            RunOptions {
                seed: Some(42),
                max_steps: Some(50),
                stdin: &mut s2,
                stdout: &mut o2,
                stderr: &mut e2,
            },
        );
        assert_eq!(o1, o2);
    }

    // ---------- v0.4 multi-IP tests ----------

    #[test]
    fn split_spawns_opposite_direction_ip() {
        // Column 0: `t.@`. IP starts at (0,0) east.
        // Tick 1: `t` at (0,0) spawns new IP at (-1,0) going west. Original IP
        //   advances to (1,0).
        // Tick 2: IP#0 at (1,0) is `.` — prints underflow 0 ("0 "). IP#1 at
        //   (-1,0) sees space → NOP, advances to (-2,0).
        // Tick 3: IP#0 at (2,0) is `@` — halts, removed. IP#1 keeps drifting
        //   west across spaces forever. Cap it.
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_source(
            "t.@",
            RunOptions {
                seed: Some(0),
                max_steps: Some(50),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(code, ExitCode::MaxSteps);
        // IP#0 printed one "0 " on tick 2 before halting on tick 3.
        assert_eq!(String::from_utf8(stdout).unwrap(), "0 ");
    }

    #[test]
    fn split_in_strmode_pushes_codepoint() {
        // Inside string mode `t` is codepoint 116, not SPLIT.
        assert_eq!(run("\"t\".@").1, "116 ");
    }

    #[test]
    fn split_twice_halts_cleanly_when_both_ips_reach_halt() {
        // Row 0: "@.t" — IP at (0,0) east. @: halts on tick 1, IP removed.
        //   Program ends without ever visiting `t`. Output: nothing.
        assert_eq!(run("@.t").1, "");
    }

    #[test]
    fn split_both_ips_write_stdout_in_birth_order() {
        //  Row 0: "1t2.@"
        //  Row 1:   "  .@"
        //  Tick 1: IP#0 at (0,0)=`1` pushes 1. Advance to (1,0).
        //  Tick 2: IP#0 at (1,0)=`t` spawns IP#1 at (0,0) going west with
        //    empty stack. IP#0 advances to (2,0).
        //  Tick 3: IP#0 at (2,0)=`2` pushes 2. Stack [1, 2]. Advance to (3,0).
        //          IP#1 at (0,0)=`1` pushes 1 to its own stack. Advance to
        //    (-1,0).
        //  Tick 4: IP#0 at (3,0)=`.` prints "2 ". IP#1 at (-1,0)=space → NOP.
        //  Tick 5: IP#0 at (4,0)=`@` halts. IP#1 at (-2,0)=space.
        //  After tick 5: ips = [IP#1]. IP#1 drifts west over spaces forever.
        //
        // Cap it; only IP#0 should have produced output.
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_source(
            "1t2.@",
            RunOptions {
                seed: Some(0),
                max_steps: Some(40),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        assert_eq!(code, ExitCode::MaxSteps);
        assert_eq!(String::from_utf8(stdout).unwrap(), "2 ");
    }

    #[test]
    fn split_per_ip_stack_independence() {
        // Source: "t5.@@"
        // Tick 1: IP#0 at (0,0)=`t` spawns IP#1 at (-1,0) going west. IP#0
        //   advances to (1,0).
        // Tick 2: IP#0 at (1,0)=`5` pushes 5. IP#1 at (-1,0)=space → NOP.
        //   IP#1 advances to (-2,0).
        // Tick 3: IP#0 at (2,0)=`.` pops 5, prints "5 ". IP#1 at (-2,0)=space.
        // Tick 4: IP#0 at (3,0)=`@` halts.
        //
        // The new IP's stack starts empty and never affects IP#0 — we
        // confirm the print is 5, not 0 (which would mean the fresh IP
        // had somehow interfered).
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_source(
            "t5.@@",
            RunOptions {
                seed: Some(0),
                max_steps: Some(40),
                stdin: &mut stdin,
                stdout: &mut stdout,
                stderr: &mut stderr,
            },
        );
        // Cap fires because IP#1 is still wandering.
        assert_eq!(code, ExitCode::MaxSteps);
        assert_eq!(String::from_utf8(stdout).unwrap(), "5 ");
    }

    // ---------- wind-speed + collision-merge tests ----------
    //
    // Wind-speed (≫/≪) and IP-collision-merge semantics from
    // SPEC §3.7 and §3.8.

    use crate::parser::parse as parse_source;

    fn build_vm(source: &str, max_steps: Option<u64>) -> Vm {
        let (grid, _) = parse_source(source);
        Vm::new(grid, Some(0), max_steps)
    }

    #[test]
    fn gust_skips_intermediate_cell() {
        // Layout: ≫9.@@  (positions 0..=4)
        // Tick 1: ≫ at (0,0). speed→2. Advance 2 → (2,0)=`.`. The
        //         `9` at (1,0) is SKIPPED — that's the whole point.
        // Tick 2: `.` pops empty stack, prints "0 ". Advance 2 →
        //         (4,0)=@.
        // Tick 3: @ halts.
        let (code, out, _) = run("≫9.@@");
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "0 ");
    }

    #[test]
    fn calm_at_speed_one_traps() {
        let (code, _, err) = run("≪@");
        assert_eq!(code, ExitCode::Trap);
        assert_eq!(code.code(), 134);
        assert!(err.contains("calm in still air"));
    }

    #[test]
    fn calm_brings_speed_back_to_one() {
        // Layout: ≫ . ≪ . @  (positions 0..=4)
        // Tick 1: ≫ at (0,0). speed→2. Advance 2 → (2,0)=≪.
        // Tick 2: ≪ at (2,0). speed→1. Advance 1 → (3,0)=`.`.
        // Tick 3: `.` prints "0 ". Advance 1 → (4,0)=@.
        // Tick 4: @ halts.
        let (code, out, _) = run("≫.≪.@");
        assert_eq!(code, ExitCode::Ok);
        assert_eq!(out, "0 ");
    }

    #[test]
    fn split_child_inherits_parent_speed() {
        // Layout: ≫ space t @  (positions 0..=3)
        // Tick 1: ≫ at (0,0). speed→2. Advance 2 → (2,0)=t.
        // Tick 2: SPLIT at (2,0). Child born at (1,0) going west,
        //         inheriting speed 2. Parent advances 2 → (4,0)=space.
        let mut vm = build_vm("≫ t @", Some(40));
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips[0].speed, BigInt::from(2));
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips.len(), 2);
        assert_eq!(vm.ips[1].speed, BigInt::from(2));
        assert_eq!((vm.ips[1].ip.dx, vm.ips[1].ip.dy), (-1, 0));
        assert_eq!((vm.ips[1].ip.x, vm.ips[1].ip.y), (1, 0));
    }

    #[test]
    fn collision_head_on_dies() {
        // Hand-construct two IPs that collide head-on after movement.
        let mut vm = build_vm("    ", Some(40));
        vm.ips[0].ip.x = 0;
        vm.ips[0].ip.dx = 1;
        vm.ips.push(IpContext {
            ip: Ip { x: 2, y: 0, dx: -1, dy: 0 },
            stack: Vec::new(),
            strmode: false,
            halted: false,
            speed: BigInt::one(),
        });
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert!(vm.halted, "head-on collision should leave no live IPs");
    }

    #[test]
    fn collision_concatenates_stacks_in_birth_order() {
        let mut vm = build_vm("    ", Some(2));
        vm.ips[0].stack = vec![BigInt::from(1), BigInt::from(2)];
        // IP#1 born south of (1,0), heading down — ends at (1,0)
        // alongside IP#0's destination (1,0) coming from (0,0) east.
        vm.ips.push(IpContext {
            ip: Ip { x: 1, y: -1, dx: 0, dy: 1 },
            stack: vec![BigInt::from(3), BigInt::from(4)],
            strmode: false,
            halted: false,
            speed: BigInt::one(),
        });
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips.len(), 1);
        assert_eq!(
            vm.ips[0].stack,
            vec![
                BigInt::from(1),
                BigInt::from(2),
                BigInt::from(3),
                BigInt::from(4),
            ],
        );
        // Direction sum: (1,0) + (0,1) = (1,1) → SE.
        assert_eq!((vm.ips[0].ip.dx, vm.ips[0].ip.dy), (1, 1));
    }

    #[test]
    fn collision_takes_max_speed() {
        let mut vm = build_vm("    ", Some(2));
        // IP#0 east speed 3 from (0,0) → ends at (3,0).
        vm.ips[0].speed = BigInt::from(3);
        // IP#1 south speed 5 from (3,-5) → ends at (3,0).
        vm.ips.push(IpContext {
            ip: Ip { x: 3, y: -5, dx: 0, dy: 1 },
            stack: Vec::new(),
            strmode: false,
            halted: false,
            speed: BigInt::from(5),
        });
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips.len(), 1);
        assert_eq!(vm.ips[0].speed, BigInt::from(5));
    }

    #[test]
    fn collision_resets_strmode() {
        let mut vm = build_vm("    ", Some(2));
        vm.ips[0].strmode = true;
        vm.ips.push(IpContext {
            ip: Ip { x: 1, y: -1, dx: 0, dy: 1 },
            stack: Vec::new(),
            strmode: false,
            halted: false,
            speed: BigInt::one(),
        });
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips.len(), 1);
        assert!(!vm.ips[0].strmode);
    }

    #[test]
    fn three_ip_merge_birth_order() {
        let mut vm = build_vm("    ", Some(2));
        // IP#0 east speed 1 from (0,0) → (1,0). Stack [1].
        vm.ips[0].stack = vec![BigInt::from(1)];
        // IP#1 east speed 2 from (-1,0) → (1,0). Stack [2].
        vm.ips.push(IpContext {
            ip: Ip { x: -1, y: 0, dx: 1, dy: 0 },
            stack: vec![BigInt::from(2)],
            strmode: false,
            halted: false,
            speed: BigInt::from(2),
        });
        // IP#2 south speed 1 from (1,-1) → (1,0). Stack [3].
        vm.ips.push(IpContext {
            ip: Ip { x: 1, y: -1, dx: 0, dy: 1 },
            stack: vec![BigInt::from(3)],
            strmode: false,
            halted: false,
            speed: BigInt::one(),
        });
        let mut stdin: &[u8] = b"";
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        vm.step(&mut stdin, &mut stdout, &mut stderr);
        assert_eq!(vm.ips.len(), 1);
        assert_eq!(
            vm.ips[0].stack,
            vec![BigInt::from(1), BigInt::from(2), BigInt::from(3)],
        );
        assert_eq!(vm.ips[0].speed, BigInt::from(2));
    }
}

#[cfg(all(test, feature = "metrics"))]
mod metrics_tests {
    use super::*;

    fn run_source_with_metrics(src: &str, max_steps: u64) -> (ExitCode, VmMetrics, String) {
        let (grid, _scan_text) = parse(src);
        let mut vm = Vm::new(grid, Some(0), Some(max_steps));
        let mut stdin: &[u8] = b"";
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let exit = vm.run(&mut stdin, &mut stdout, &mut stderr);
        (exit, vm.metrics, String::from_utf8_lossy(&stdout).to_string())
    }

    #[test]
    fn metrics_default_is_minimal_baseline() {
        let m = VmMetrics::default();
        assert_eq!(m.spawned_ips, 0);
        assert_eq!(m.grid_writes, 0);
        assert_eq!(m.branch_count, 0);
        assert_eq!(m.hard_opcode_bitmap, 0);
        // Vm::new() bumps max_alive_ips to 1, but the bare default is 0.
        assert_eq!(m.max_alive_ips, 0);
    }

    #[test]
    fn metrics_root_ip_visible_after_first_tick() {
        // Trivial program: just `@`. Halt fires on tick 0; the
        // post-tick max_alive_ips snapshot picks up the steady-state
        // count of zero (the only IP halted), but the floor we set in
        // Vm::new() keeps max_alive_ips at 1.
        let (_, m, _) = run_source_with_metrics("@", 100);
        assert_eq!(m.max_alive_ips, 1);
        assert_eq!(m.spawned_ips, 0);
    }

    #[test]
    fn metrics_strmode_bit_set_for_hello() {
        // hello.wnd shape: opens string mode, pushes 13 chars, halts.
        let (_, m, _) = run_source_with_metrics(
            "\"!dlroW ,olleH\",,,,,,,,,,,,,@",
            10_000,
        );
        assert_ne!(m.hard_opcode_bitmap & VmMetrics::BIT_STRMODE, 0);
        assert_eq!(m.spawned_ips, 0);
        assert_eq!(m.grid_writes, 0);
        assert_eq!(m.branch_count, 0);
    }

    #[test]
    fn metrics_split_increments_spawn_and_peak() {
        // puzzle_hard.wnd: two SPLITs, peak 4 IPs, no other hard ops.
        let (_, m, _) = run_source_with_metrics("→1.2.3t4.5t6.7←@", 100);
        assert_eq!(m.spawned_ips, 3);  // t fires 3 times across the run
        assert_eq!(m.max_alive_ips, 4);
        assert_eq!(m.grid_writes, 0);
        assert_eq!(m.branch_count, 0);
        assert_eq!(m.hard_opcode_bitmap, VmMetrics::BIT_T);
    }

    #[test]
    fn metrics_grid_writes_and_branches_track_independently() {
        // Minimal program that fires `p` and `_` exactly once each
        // before halting:
        //   1 0 0 p   push v=1, x=0, y=0; pop them all in p; grid[0,0] := 1
        //   0 _       push 0; IF_H pops 0 → keep heading east
        //   @         halt
        // The IP only walks east, so no cell is revisited and the
        // counts are guaranteed to be 1.
        let (exit, m, _) = run_source_with_metrics("100p0_@", 100);
        assert_eq!(exit, ExitCode::Ok);
        assert_eq!(m.grid_writes, 1);
        assert_eq!(m.branch_count, 1);
        assert_ne!(m.hard_opcode_bitmap & VmMetrics::BIT_P, 0);
        assert_ne!(m.hard_opcode_bitmap & VmMetrics::BIT_IFH, 0);
    }
}
