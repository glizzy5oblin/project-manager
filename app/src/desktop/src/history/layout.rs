//! Lane assignment for the branch graph. Pure: works on row indices only, so it is tested
//! without git.
//!
//! Input: commits in display order (every parent below its children) and, per row, the rows of
//! its parents. Output: the lane (column) of each commit and, per row, the line segments from that
//! row's centre to the next row's centre.
//!
//! Rules, close to `git log --graph`:
//! - A commit takes the leftmost lane waiting for it; other lanes waiting for it merge into it.
//!   A commit nobody waits for (a branch tip) takes the leftmost free lane.
//! - The first parent continues in the commit's lane, even when another lane already waits for
//!   it, so a branch runs beside its base until the fork point.
//! - Other parents (merge sources) join a lane already waiting for them, or open a new lane,
//!   preferably to the right of the commit.
//! - Lanes are never shifted; freed lanes are reused, so lines stay straight.
//! - Each lane run gets a colour index when it opens and keeps it to the end, except at a
//!   recoloured row: the lane continues there in a new colour (so a branch that shares its lane
//!   with the branch it was made from still looks different from it), or in colour 0 for the
//!   main branch's own commits. Colours count up and wrap around the theme's palette; when
//!   colour 0 is reserved for the main branch, other lanes never get it.

/// Colours in the theme's graph palette (`--graph-1..8`); colour index `c` is `--graph-(c % 8 + 1)`.
pub const PALETTE: u16 = 8;

/// What a row does to the colour of its lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recolor {
    /// Keep the lane's colour.
    Keep,
    /// The commit, and what lies below it in its lane, takes a new colour.
    New,
    /// The commit is on the main branch: colour 0.
    Main,
}

/// A line segment from (`from`, row) to (`to`, row + 1), in lane units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub from: u16,
    pub to: u16,
    pub color: u16,
}

#[derive(Debug, Default)]
pub struct Layout {
    /// Lane of each row's commit.
    pub lane: Vec<u16>,
    /// Colour index of each row's commit.
    pub color: Vec<u16>,
    /// Row `r`'s segments are `edges[edge_start[r]..edge_start[r + 1]]`.
    pub edge_start: Vec<u32>,
    pub edges: Vec<Edge>,
    /// Widest row, in lanes.
    pub max_lanes: u16,
}

impl Layout {
    pub fn edges_of(&self, row: usize) -> &[Edge] {
        &self.edges[self.edge_start[row] as usize..self.edge_start[row + 1] as usize]
    }

    /// Lanes a row needs: its commit and every segment touching it from above or below.
    pub fn width_of(&self, row: usize) -> u16 {
        let mut w = self.lane[row] + 1;
        for e in self.edges_of(row) {
            w = w.max(e.from + 1).max(e.to + 1);
        }
        if row > 0 {
            for e in self.edges_of(row - 1) {
                w = w.max(e.to + 1);
            }
        }
        w
    }
}

/// Lay out `rows` commits. `parents(r)` gives the parent rows of row `r`; each must be greater
/// than `r` (parents are listed below their children). Parents outside the set are left out by
/// the caller.
pub fn layout(rows: usize, parents: impl Fn(usize) -> Vec<u32>) -> Layout {
    layout_with(rows, parents, false, |_| Recolor::Keep)
}

/// [`layout`] with rows that recolour their lane. The segment arriving from above keeps the old
/// colour. With `reserve_main`, only `Recolor::Main` rows get colour 0.
pub fn layout_with(
    rows: usize,
    parents: impl Fn(usize) -> Vec<u32>,
    reserve_main: bool,
    recolor: impl Fn(usize) -> Recolor,
) -> Layout {
    let mut out = Layout {
        lane: Vec::with_capacity(rows),
        color: Vec::with_capacity(rows),
        edge_start: Vec::with_capacity(rows + 1),
        edges: Vec::new(),
        max_lanes: 0,
    };
    // What each lane waits for (a row below the current one), where its segment starts in the
    // current row, and its colour.
    let mut waits: Vec<Option<u32>> = Vec::new();
    let mut origin: Vec<u16> = Vec::new();
    let mut colors: Vec<u16> = Vec::new();
    // Segments from a commit into a lane that already waits for one of its merge parents:
    // (from lane, into lane, colour).
    let mut joins: Vec<(u16, u16, u16)> = Vec::new();
    let mut next_color: u16 = 0;
    let mut new_color = || {
        if reserve_main && next_color.is_multiple_of(PALETTE) {
            next_color = next_color.wrapping_add(1);
        }
        let c = next_color;
        next_color = next_color.wrapping_add(1);
        c
    };

    out.edge_start.push(0);
    for r in 0..rows {
        let r32 = r as u32;
        let node = match waits.iter().position(|w| *w == Some(r32)) {
            Some(l) => l,
            None => {
                let l = waits.iter().position(Option::is_none).unwrap_or_else(|| {
                    waits.push(None);
                    origin.push(0);
                    colors.push(0);
                    waits.len() - 1
                });
                colors[l] = new_color();
                l
            }
        };
        let node16 = node as u16;

        // Segments of the previous row, now that this row's commit lane is known.
        if r > 0 {
            for (l, w) in waits.iter().enumerate() {
                if let Some(target) = w {
                    let to = if *target == r32 { node16 } else { l as u16 };
                    out.edges.push(Edge {
                        from: origin[l],
                        to,
                        color: colors[l],
                    });
                }
            }
            for &(from, into, color) in &joins {
                let to = if waits[into as usize] == Some(r32) {
                    node16
                } else {
                    into
                };
                out.edges.push(Edge { from, to, color });
            }
            out.edge_start.push(out.edges.len() as u32);
        }
        joins.clear();
        match recolor(r) {
            Recolor::Keep => {}
            Recolor::New => colors[node] = new_color(),
            Recolor::Main => colors[node] = 0,
        }

        out.lane.push(node16);
        out.color.push(colors[node]);

        for w in waits.iter_mut() {
            if *w == Some(r32) {
                *w = None;
            }
        }
        for (l, o) in origin.iter_mut().enumerate() {
            *o = l as u16;
        }

        for (i, p) in parents(r).into_iter().enumerate() {
            debug_assert!(p > r32, "parent row {p} is not below row {r}");
            if i == 0 {
                waits[node] = Some(p);
                continue;
            }
            if let Some(l) = waits.iter().position(|w| *w == Some(p)) {
                if l != node {
                    joins.push((node16, l as u16, colors[l]));
                }
                continue;
            }
            let l = (node + 1..waits.len())
                .find(|&l| waits[l].is_none())
                .or_else(|| waits.iter().position(Option::is_none))
                .unwrap_or_else(|| {
                    waits.push(None);
                    origin.push(0);
                    colors.push(0);
                    waits.len() - 1
                });
            waits[l] = Some(p);
            origin[l] = node16;
            colors[l] = new_color();
        }

        while waits.last() == Some(&None) {
            waits.pop();
            origin.pop();
            colors.pop();
        }
        out.max_lanes = out.max_lanes.max(waits.len() as u16).max(node16 + 1);
    }
    // The last row has no segments below it.
    if rows > 0 {
        out.edge_start.push(out.edges.len() as u32);
    }
    for e in &out.edges {
        out.max_lanes = out.max_lanes.max(e.from + 1).max(e.to + 1);
    }
    out
}
