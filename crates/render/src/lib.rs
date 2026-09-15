//! Renderer: decides *when* and *what* to push to the e-ink panel.
//!
//! Every tick it compares the wanted grid (terminal snapshot + cursor) with the
//! shadow grid (what is on the glass). Nothing is refreshed until the wanted
//! grid has been quiet for `quiet_ms`, or `max_latency_ms` has elapsed since the
//! first unflushed change. That gives a single keystroke a fast echo and caps
//! streaming output at roughly `1000 / max_latency_ms` refreshes per second no
//! matter how fast bytes arrive.
//!
//! Dirty cells are grouped into row bands, one refresh per band. If the dirty
//! area covers more than `full_threshold` of the screen, one whole-screen Fast
//! refresh is issued instead. After `ghost_budget` partial refreshes, or when the
//! screen goes idle with enough partials accumulated, a Full refresh clears
//! ghosting.

use panel::{Cell, CellRect, Panel, Waveform};
use term::Grid;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Flush once the wanted grid has not changed for this long.
    pub quiet_ms: u64,
    /// Flush anyway once the oldest unflushed change is this old.
    pub max_latency_ms: u64,
    /// Fraction of screen rows dirty above which a whole-screen refresh is used.
    pub full_threshold: f32,
    /// Force a Full refresh after this many partial refreshes.
    pub ghost_budget: u32,
    /// When idle this long with at least `ghost_idle_min_partials`, do a Full refresh.
    pub ghost_idle_ms: u64,
    pub ghost_idle_min_partials: u32,
    /// Re-push the whole grid with a Fast refresh if nothing was refreshed for
    /// this long. Cheap on e-ink (unchanged pixels do not move) and guarantees
    /// the glass never lags the model by more than this. 0 disables.
    pub heartbeat_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            quiet_ms: 50,
            max_latency_ms: 250,
            full_threshold: 0.5,
            ghost_budget: 40,
            ghost_idle_ms: 4000,
            ghost_idle_min_partials: 6,
            heartbeat_ms: 1000,
        }
    }
}

pub struct Renderer {
    cfg: Config,
    shadow: Grid,
    last_wanted: Grid,
    pending_since: Option<u64>,
    last_change: u64,
    partials_since_full: u32,
    last_flush: u64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Flushed {
    Nothing,
    Partial(u32),
    Full,
}

impl Renderer {
    pub fn new(cfg: Config, cols: u16, rows: u16) -> Self {
        Renderer {
            cfg,
            shadow: Grid::blank(cols, rows),
            last_wanted: Grid::blank(cols, rows),
            pending_since: None,
            last_change: 0,
            partials_since_full: 0,
            last_flush: 0,
        }
    }

    pub fn partials_since_full(&self) -> u32 {
        self.partials_since_full
    }

    /// Apply the cursor as an inverse cell so it is diffed like any other change.
    fn wanted(grid: &Grid) -> Grid {
        let mut w = grid.clone();
        if let Some((c, r)) = grid.cursor {
            if c < w.cols && r < w.rows {
                let cell = w.cell_mut(c, r);
                cell.inverse = !cell.inverse;
            }
        }
        w
    }

    /// Call frequently (every 5 to 20 ms) with a monotonic clock in ms.
    pub fn tick(&mut self, now: u64, grid: &Grid, panel: &mut dyn Panel) -> Flushed {
        let wanted = Self::wanted(grid);
        if wanted != self.last_wanted {
            self.last_change = now;
            if self.pending_since.is_none() {
                self.pending_since = Some(now);
            }
            self.last_wanted = wanted;
        }

        let Some(since) = self.pending_since else {
            return self.maybe_idle_full(now, panel);
        };
        let quiet = now.saturating_sub(self.last_change) >= self.cfg.quiet_ms;
        let overdue = now.saturating_sub(since) >= self.cfg.max_latency_ms;
        if !(quiet || overdue) {
            return Flushed::Nothing;
        }
        self.flush(now, panel)
    }

    fn maybe_idle_full(&mut self, now: u64, panel: &mut dyn Panel) -> Flushed {
        if self.partials_since_full >= self.cfg.ghost_idle_min_partials
            && now.saturating_sub(self.last_flush) >= self.cfg.ghost_idle_ms
        {
            self.full(now, panel);
            return Flushed::Full;
        }
        if self.cfg.heartbeat_ms > 0 && now.saturating_sub(self.last_flush) >= self.cfg.heartbeat_ms {
            let (cols, rows) = (self.shadow.cols, self.shadow.rows);
            panel.refresh(CellRect { col: 0, row: 0, cols, rows }, Waveform::Fast);
            self.last_flush = now;
            return Flushed::Partial(1);
        }
        Flushed::Nothing
    }

    /// Draw every cell of `grid` and push one flashing Full refresh. Use after
    /// the layout changed underneath (keyboard shown/hidden).
    pub fn redraw_full(&mut self, now: u64, grid: &Grid, panel: &mut dyn Panel) {
        let wanted = Self::wanted(grid);
        for r in 0..wanted.rows {
            for c in 0..wanted.cols {
                panel.draw(c, r, wanted.cell(c, r));
            }
        }
        panel.refresh(CellRect { col: 0, row: 0, cols: wanted.cols, rows: wanted.rows }, Waveform::Full);
        self.shadow = wanted.clone();
        self.last_wanted = wanted;
        self.pending_since = None;
        self.partials_since_full = 0;
        self.last_flush = now;
    }

    fn full(&mut self, now: u64, panel: &mut dyn Panel) {
        let (cols, rows) = (self.shadow.cols, self.shadow.rows);
        for r in 0..rows {
            for c in 0..cols {
                panel.draw(c, r, self.shadow.cell(c, r));
            }
        }
        panel.refresh(CellRect { col: 0, row: 0, cols, rows }, Waveform::Full);
        self.partials_since_full = 0;
        self.last_flush = now;
    }

    fn flush(&mut self, now: u64, panel: &mut dyn Panel) -> Flushed {
        let wanted = std::mem::replace(&mut self.last_wanted, Grid::blank(0, 0));
        let (cols, rows) = (wanted.cols, wanted.rows);

        // Per-row dirty span.
        let mut spans: Vec<Option<(u16, u16)>> = vec![None; rows as usize];
        for r in 0..rows {
            for c in 0..cols {
                let want: &Cell = wanted.cell(c, r);
                if want != self.shadow.cell(c, r) {
                    panel.draw(c, r, want);
                    let s = &mut spans[r as usize];
                    *s = Some(match *s {
                        None => (c, c),
                        Some((lo, hi)) => (lo.min(c), hi.max(c)),
                    });
                }
            }
        }
        self.shadow = wanted.clone();
        self.last_wanted = wanted;
        self.pending_since = None;
        self.last_flush = now;

        let dirty_rows = spans.iter().filter(|s| s.is_some()).count();
        if dirty_rows == 0 {
            return Flushed::Nothing;
        }

        if self.partials_since_full >= self.cfg.ghost_budget {
            panel.refresh(CellRect { col: 0, row: 0, cols, rows }, Waveform::Full);
            self.partials_since_full = 0;
            return Flushed::Full;
        }

        if dirty_rows as f32 / rows as f32 > self.cfg.full_threshold {
            panel.refresh(CellRect { col: 0, row: 0, cols, rows }, Waveform::Fast);
            self.partials_since_full += 1;
            return Flushed::Partial(1);
        }

        // Merge runs of consecutive dirty rows into bands.
        let mut bands: Vec<CellRect> = Vec::new();
        let mut cur: Option<(u16, u16, u16, u16)> = None; // (row0, row1, lo, hi)
        for (r, s) in spans.iter().enumerate() {
            match (s, cur) {
                (Some((lo, hi)), Some((r0, r1, blo, bhi))) if r1 + 1 == r as u16 => {
                    cur = Some((r0, r as u16, blo.min(*lo), bhi.max(*hi)));
                }
                (Some((lo, hi)), Some((r0, r1, blo, bhi))) => {
                    bands.push(CellRect { col: blo, row: r0, cols: bhi - blo + 1, rows: r1 - r0 + 1 });
                    cur = Some((r as u16, r as u16, *lo, *hi));
                }
                (Some((lo, hi)), None) => cur = Some((r as u16, r as u16, *lo, *hi)),
                (None, _) => {}
            }
        }
        if let Some((r0, r1, lo, hi)) = cur {
            bands.push(CellRect { col: lo, row: r0, cols: hi - lo + 1, rows: r1 - r0 + 1 });
        }
        let n = bands.len() as u32;
        for b in bands {
            panel.refresh(b, Waveform::Fast);
        }
        self.partials_since_full += n;
        Flushed::Partial(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel::FakePanel;
    use term::Terminal;

    struct Sim {
        term: Terminal,
        panel: FakePanel,
        r: Renderer,
        now: u64,
    }

    impl Sim {
        fn new(cols: u16, rows: u16, cfg: Config) -> Self {
            let cfg = Config { heartbeat_ms: 0, ..cfg };
            Sim { term: Terminal::new(cols, rows, 0), panel: FakePanel::new(cols, rows), r: Renderer::new(cfg, cols, rows), now: 0 }
        }
        fn feed(&mut self, b: &[u8]) {
            self.term.feed(b);
        }
        /// Advance `ms`, ticking every 5 ms.
        fn run(&mut self, ms: u64) {
            let end = self.now + ms;
            while self.now < end {
                self.now += 5;
                let g = self.term.snapshot();
                self.r.tick(self.now, &g, &mut self.panel);
            }
        }
    }

    #[test]
    fn keystroke_echo_flushes_after_quiet_period_with_a_tiny_rect() {
        let mut s = Sim::new(60, 20, Config::default());
        s.run(100); // startup: the cursor itself is drawn once, then nothing
        assert_eq!(s.panel.total_refreshes(), 1);
        s.run(1000);
        assert_eq!(s.panel.total_refreshes(), 1, "idle screen must not refresh");
        s.feed(b"a");
        s.run(40);
        assert_eq!(s.panel.total_refreshes(), 1, "must not flush before quiet_ms");
        s.run(30);
        assert_eq!(s.panel.total_refreshes(), 2);
        let (rect, wf) = s.panel.refreshes[1];
        assert_eq!(wf, Waveform::Fast);
        assert_eq!(rect.rows, 1);
        assert!(rect.cols <= 2, "cell plus cursor only, got {rect:?}");
        assert_eq!(s.panel.row_text(0), "a");
    }

    /// The first-class metric: refreshes per second of streaming output.
    #[test]
    fn streaming_output_is_capped_by_max_latency() {
        let cfg = Config::default();
        let mut s = Sim::new(60, 20, cfg);
        let seconds = 5;
        // 50 lines per second, far more than e-ink can show.
        for i in 0..(seconds * 50) {
            s.feed(format!("line {i} lorem ipsum dolor sit amet\r\n").as_bytes());
            s.run(20);
        }
        let per_sec = s.panel.total_refreshes() as f32 / seconds as f32;
        let cap = 1000.0 / cfg.max_latency_ms as f32;
        assert!(per_sec <= cap + 0.5, "{per_sec} refreshes/s exceeds cap {cap}");
        assert!(per_sec >= 2.0, "{per_sec} refreshes/s: output is not visibly progressing");
        // Scrolling dirties every row, so these should all be single whole-screen refreshes.
        let whole = s.panel.refreshes.iter().filter(|(r, _)| r.rows == 20).count();
        assert_eq!(whole, s.panel.total_refreshes());
        // Let the tail flush, then check the glass shows the final state.
        s.run(300);
        assert_eq!(s.panel.row_text(18), format!("line {} lorem ipsum dolor sit amet", seconds * 50 - 1));
    }

    #[test]
    fn spinner_in_one_cell_stays_a_one_row_refresh() {
        let mut s = Sim::new(60, 20, Config::default());
        s.feed(b"working ");
        s.run(100);
        let before = s.panel.total_refreshes();
        for _ in 0..25 {
            for f in [b"\x08|", b"\x08/", b"\x08-", b"\x08\\"] {
                s.feed(f);
                s.run(10);
            }
        }
        let spins = &s.panel.refreshes[before..];
        assert!(spins.len() <= 5, "spinner caused {} refreshes in 1 s", spins.len());
        assert!(spins.iter().all(|(r, _)| r.rows == 1 && r.cols <= 3), "{spins:?}");
    }

    #[test]
    fn ghost_budget_forces_a_full_refresh() {
        let cfg = Config { ghost_budget: 10, ghost_idle_ms: 1_000_000, ..Config::default() };
        let mut s = Sim::new(60, 20, cfg);
        for i in 0..15 {
            s.feed(format!("{}", i % 10).as_bytes());
            s.run(100);
        }
        assert_eq!(s.panel.count(Waveform::Full), 1);
        assert!(s.r.partials_since_full() < 10);
    }

    #[test]
    fn idle_screen_with_ghosting_gets_one_full_refresh_then_rests() {
        let cfg = Config { ghost_idle_ms: 500, ghost_idle_min_partials: 3, ..Config::default() };
        let mut s = Sim::new(60, 20, cfg);
        for _ in 0..4 {
            s.feed(b"x");
            s.run(100);
        }
        assert_eq!(s.panel.count(Waveform::Full), 0);
        s.run(600);
        assert_eq!(s.panel.count(Waveform::Full), 1);
        s.run(5000);
        assert_eq!(s.panel.count(Waveform::Full), 1, "idle full refresh must not repeat");
        assert_eq!(s.panel.row_text(0), "xxxx");
    }

    #[test]
    fn heartbeat_repushes_the_grid_once_a_second_when_idle() {
        let mut s = Sim::new(60, 20, Config::default());
        s.r = Renderer::new(Config { heartbeat_ms: 1000, ghost_idle_ms: 1_000_000, ..Config::default() }, 60, 20);
        s.feed(b"x");
        s.run(100);
        let before = s.panel.total_refreshes();
        s.run(3050);
        let hb = s.panel.total_refreshes() - before;
        assert_eq!(hb, 3, "expected one heartbeat per idle second");
        assert!(s.panel.refreshes[before..].iter().all(|(r, w)| *w == Waveform::Fast && r.rows == 20));
    }

    #[test]
    fn two_separate_dirty_rows_become_two_bands() {
        let mut s = Sim::new(60, 20, Config::default());
        s.feed(b"\x1b[?25l"); // hide cursor to keep the diff exact
        s.run(100);
        s.feed(b"\x1b[1;1Ha\x1b[10;1Hb");
        s.run(100);
        let fast: Vec<_> = s.panel.refreshes.iter().filter(|(_, w)| *w == Waveform::Fast).collect();
        assert_eq!(fast.len(), 2, "{fast:?}");
        assert_eq!(fast[0].0, CellRect { col: 0, row: 0, cols: 1, rows: 1 });
        assert_eq!(fast[1].0, CellRect { col: 0, row: 9, cols: 1, rows: 1 });
    }
}
