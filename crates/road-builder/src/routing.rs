use crate::{
    document::RoutingSettings,
    geometry::{Point, norm, sub},
    terrain::Terrain,
};
use anyhow::{Result, bail, ensure};
use std::{
    cmp::Ordering,
    f64::consts::{FRAC_PI_2, FRAC_PI_4, TAU},
    collections::{BinaryHeap, HashMap},
    sync::atomic::{AtomicBool, Ordering as AO},
};

pub fn inside(p: Point, poly: &[Point]) -> bool {
    if poly.len() < 3 {
        return false;
    }
    let mut hit = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[j];
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            hit = !hit;
        }
        j = i;
    }
    hit
}
pub fn blocked_segment(a: Point, b: Point, polygons: &[Vec<Point>]) -> bool {
    fn cross(a: Point, b: Point, c: Point) -> f64 {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }
    fn intersects(a: Point, b: Point, c: Point, d: Point) -> bool {
        if a[0].max(b[0]) < c[0].min(d[0])
            || c[0].max(d[0]) < a[0].min(b[0])
            || a[1].max(b[1]) < c[1].min(d[1])
            || c[1].max(d[1]) < a[1].min(b[1])
        {
            return false;
        }
        cross(a, b, c) * cross(a, b, d) <= 0. && cross(c, d, a) * cross(c, d, b) <= 0.
    }
    polygons.iter().any(|p| {
        p.len() >= 3
            && (inside(a, p)
                || inside(b, p)
                || p.iter()
                    .copied()
                    .zip(p.iter().copied().cycle().skip(1))
                    .take(p.len())
                    .any(|(c, d)| intersects(a, b, c, d)))
    })
}
/// Shortest distance between a segment and a polygon outline; zero when they touch.
fn polygon_distance(a: Point, b: Point, poly: &[Point]) -> f64 {
    fn point_segment(p: Point, a: Point, b: Point) -> f64 {
        let d = sub(b, a);
        let l = d[0] * d[0] + d[1] * d[1];
        let t = if l < 1e-12 {
            0.
        } else {
            (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / l).clamp(0., 1.)
        };
        norm(sub(p, [a[0] + d[0] * t, a[1] + d[1] * t]))
    }
    if blocked_segment(a, b, std::slice::from_ref(&poly.to_vec())) {
        return 0.;
    }
    poly.iter()
        .copied()
        .zip(poly.iter().copied().cycle().skip(1))
        .take(poly.len())
        .map(|(c, d)| {
            point_segment(a, c, d)
                .min(point_segment(b, c, d))
                .min(point_segment(c, a, b))
                .min(point_segment(d, a, b))
        })
        .fold(f64::INFINITY, f64::min)
}
/// Check a polyline against forbidden areas (kept `clearance` metres away), NODATA and
/// the maximum grade between terrain samples. Returns the total climb `Σ|Δz|`.
fn profile(
    line: &[Point],
    terrain: &Terrain,
    settings: &RoutingSettings,
    forbidden: &[Vec<Point>],
    clearance: f64,
) -> Option<f64> {
    let spacing = (terrain.meta.cell / 2.).clamp(0.1, 5.);
    let mut climb = 0.;
    let mut last: Option<f64> = None;
    for (index, w) in line.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        if blocked_segment(a, b, forbidden)
            || (clearance > 0.
                && forbidden
                    .iter()
                    .any(|poly| poly.len() >= 3 && polygon_distance(a, b, poly) < clearance))
        {
            return None;
        }
        let dist = norm(sub(b, a));
        let steps = (dist / spacing).ceil().max(1.) as usize;
        for i in usize::from(index > 0)..=steps {
            let f = i as f64 / steps as f64;
            let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
            if forbidden.iter().any(|poly| inside(p, poly)) {
                return None;
            }
            let z = terrain.sample(p)?;
            if let Some(old) = last
                && dist > 1e-8
            {
                if (z - old).abs() / (dist / steps as f64) * 100. > settings.max_grade + 1e-6 {
                    return None;
                }
                climb += (z - old).abs();
            }
            last = Some(z);
        }
    }
    Some(climb)
}
pub fn permitted(
    a: Point,
    b: Point,
    terrain: &Terrain,
    settings: &RoutingSettings,
    forbidden: &[Vec<Point>],
) -> bool {
    profile(&[a, b], terrain, settings, forbidden, 0.).is_some()
}
#[derive(Clone, Copy)]
struct Node {
    cost: f64,
    index: usize,
}
impl PartialEq for Node {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost && self.index == o.index
    }
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost).then(o.index.cmp(&self.index))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
/// Search state: a pose on the road; `stage` is 1 once the leg's target is reached.
#[derive(Clone, Copy)]
struct State {
    p: Point,
    heading: f64,
    stage: usize,
    g: f64,
    parent: usize,
    /// Arc from the parent: heading change and length.
    delta: f64,
    length: f64,
    /// Which route arriving at the leg's start this state continues.
    origin: usize,
}
/// Sample a circular arc (a straight line when `delta` is zero) starting at `p`.
fn arc(p: Point, heading: f64, delta: f64, length: f64) -> Vec<Point> {
    let at = |t: f64| {
        if delta.abs() < 1e-9 {
            [p[0] + t * heading.cos(), p[1] + t * heading.sin()]
        } else {
            let r = length / delta;
            let phi = heading + delta * t / length;
            [
                p[0] + r * (phi.sin() - heading.sin()),
                p[1] + r * (heading.cos() - phi.cos()),
            ]
        }
    };
    let n = (delta.abs() / 2f64.to_radians()).ceil().max(1.) as usize;
    (0..=n).map(|i| at(length * i as f64 / n as f64)).collect()
}
/// Single tangent-continuous arc from a pose exactly through `target`, if its radius is allowed.
fn connect(p: Point, heading: f64, target: Point, min_radius: f64) -> Option<(f64, f64)> {
    let v = sub(target, p);
    let c = norm(v);
    if c < 1e-9 {
        return Some((0., 0.));
    }
    let h = [heading.cos(), heading.sin()];
    let alpha = (h[0] * v[1] - h[1] * v[0]).atan2(h[0] * v[0] + h[1] * v[1]);
    if alpha.abs() < 1e-9 {
        return Some((0., c));
    }
    if alpha.abs() >= FRAC_PI_2 {
        return None;
    }
    let r = c / (2. * alpha.abs().sin());
    (r >= min_radius - 1e-6).then_some((2. * alpha, r * 2. * alpha.abs()))
}

/// Per-leg cost-to-go over a coarse 8-connected grid that respects forbidden areas and
/// NODATA. Grade is relaxed: a grid step costs at least its climb divided by the maximum
/// grade, which is the minimum road length any shape of climb (serpentines, a spiral,
/// a detour) needs. This steers the search towards gentle ground and long enough climbs;
/// values are scaled down to stay close to a lower bound of the arc search.
struct CostField {
    origin: Point,
    resolution: f64,
    cols: usize,
    rows: usize,
    legs: Vec<Vec<f64>>,
}
impl CostField {
    fn new(
        terrain: &Terrain,
        waypoints: &[Point],
        s: &RoutingSettings,
        forbidden: &[Vec<Point>],
        clearance: f64,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let m = &terrain.meta;
        let relaxed = RoutingSettings {
            max_grade: f64::INFINITY,
            ..s.clone()
        };
        let (width, height) = (m.cols as f64 * m.cell, m.rows as f64 * m.cell);
        let mut resolution = s.cell.max(m.cell);
        while (width / resolution).ceil() * (height / resolution).ceil() > 1_000_000. {
            resolution *= 1.25;
        }
        let (cols, rows) = (
            (width / resolution).ceil() as usize,
            (height / resolution).ceil() as usize,
        );
        let mut field = Self {
            origin: [m.east, m.north],
            resolution,
            cols,
            rows,
            legs: Vec::new(),
        };
        let center = |i: usize| {
            [
                m.east + ((i % cols) as f64 + 0.5) * resolution,
                m.north + ((i / cols) as f64 + 0.5) * resolution,
            ]
        };
        for target in &waypoints[1..] {
            let mut cost = vec![f64::INFINITY; cols * rows];
            let mut heap = BinaryHeap::new();
            if let Some(i) = field.cell(*target) {
                cost[i] = norm(sub(center(i), *target));
                heap.push(Node { cost: cost[i], index: i });
            }
            let mut pops = 0usize;
            while let Some(Node { cost: g, index }) = heap.pop() {
                pops += 1;
                if pops.is_multiple_of(4096) {
                    ensure!(!cancel.load(AO::Relaxed), "Wyznaczanie anulowane");
                }
                if g > cost[index] {
                    continue;
                }
                let (x, y) = ((index % cols) as i64, (index / cols) as i64);
                for (dx, dy) in [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1)] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= cols as i64 || ny >= rows as i64 {
                        continue;
                    }
                    let next = ny as usize * cols + nx as usize;
                    let (a, b) = (center(index), center(next));
                    if let Some(climb) = profile(&[b, a], terrain, &relaxed, forbidden, clearance) {
                        let ng = g
                            + norm(sub(b, a)).max(climb / (s.max_grade / 100.))
                            + s.slope_weight * climb;
                        if ng < cost[next] {
                            cost[next] = ng;
                            heap.push(Node { cost: ng, index: next });
                        }
                    }
                }
            }
            field.legs.push(cost);
        }
        Ok(field)
    }
    fn cell(&self, p: Point) -> Option<usize> {
        let x = ((p[0] - self.origin[0]) / self.resolution).floor();
        let y = ((p[1] - self.origin[1]) / self.resolution).floor();
        (x >= 0. && y >= 0. && (x as usize) < self.cols && (y as usize) < self.rows)
            .then(|| y as usize * self.cols + x as usize)
    }
    /// Estimated cost from `p` to the end of leg `stage`; 0 where the grid has no answer.
    fn estimate(&self, stage: usize, p: Point) -> f64 {
        // Octile paths are up to ~8 % longer than straight ones; the cell offset is subtracted.
        self.cell(p)
            .map(|i| self.legs[stage][i])
            .filter(|v| v.is_finite())
            .map_or(0., |v| (v / 1.0824 - self.resolution).max(0.))
    }
}

/// Terrain route through `waypoints` fitted with models of `family`. The fitted models may
/// deviate from the searched line by up to `tolerance`, so the search first keeps a 10 %
/// grade margin; without a fitting result it uses the full grade. The models themselves are
/// always checked against the full settings.
#[allow(clippy::too_many_arguments)]
pub fn plan(
    terrain: &Terrain,
    waypoints: &[Point],
    family: &str,
    catalog: &[crate::roads::CatalogPart],
    s: &RoutingSettings,
    forbidden: &[Vec<Point>],
    cancel: &AtomicBool,
) -> Result<Vec<crate::roads::PlacedPart>> {
    let allowed = |a, b| !cancel.load(AO::Relaxed) && permitted(a, b, terrain, s, forbidden);
    let mut error = None;
    for margin in [0.9, 1.] {
        let search = RoutingSettings {
            max_grade: s.max_grade * margin,
            ..s.clone()
        };
        let attempt = find(terrain, waypoints, &search, forbidden, cancel)
            .and_then(|path| crate::roads::fit_path(&path, family, catalog, s, allowed));
        ensure!(!cancel.load(AO::Relaxed), "Wyznaczanie anulowane");
        match attempt {
            Ok(parts) => return Ok(parts),
            Err(e) => error = Some(e),
        }
    }
    Err(error.unwrap())
}

/// Hybrid A* over the terrain. The road is a chain of arcs with a radius never below
/// `min_radius`; it passes exactly through every waypoint with a continuous heading,
/// keeps `tolerance` metres away from forbidden areas, never exceeds `max_grade` and
/// minimises length + `slope_weight` × climb. `cell` is the search step.
pub fn find(
    terrain: &Terrain,
    waypoints: &[Point],
    s: &RoutingSettings,
    forbidden: &[Vec<Point>],
    cancel: &AtomicBool,
) -> Result<Vec<Point>> {
    ensure!(waypoints.len() >= 2, "Wskaż przynajmniej dwa punkty");
    ensure!(
        [s.cell, s.max_grade, s.slope_weight, s.min_radius, s.tolerance]
            .iter()
            .all(|v| v.is_finite())
            && s.cell > 0.
            && s.max_grade > 0.
            && s.slope_weight >= 0.
            && s.min_radius > 0.
            && s.tolerance > 0.,
        "Nieprawidłowe ustawienia automatu"
    );
    let clearance = s.tolerance;
    for w in waypoints {
        ensure!(terrain.sample(*w).is_some(), "Punkty poza ASC lub w NODATA");
        ensure!(
            profile(&[*w, *w], terrain, s, forbidden, clearance).is_some(),
            "Punkt trasy leży w obszarze zakazanym lub bliżej niego niż tolerancja trasy"
        );
    }
    ensure!(
        waypoints.windows(2).all(|w| norm(sub(w[1], w[0])) > 1e-6),
        "Usuń powtórzone punkty trasy"
    );
    let field = CostField::new(terrain, waypoints, s, forbidden, clearance, cancel)?;
    // Legs are searched one after another. Each leg keeps several ways of arriving at its
    // target (different headings); the next leg starts from all of them with their costs,
    // so it picks the combination that suits it, and the heading stays continuous through
    // every intermediate waypoint.
    let mut paths = vec![vec![waypoints[0]]];
    let mut starts: Vec<Start> = Vec::new();
    for stage in 0..waypoints.len() - 1 {
        let arrivals = leg(terrain, waypoints, stage, &starts, s, forbidden, &field, cancel)?;
        starts = arrivals
            .iter()
            .enumerate()
            .map(|(i, a)| Start { heading: a.heading, g: a.g, origin: i })
            .collect();
        paths = arrivals
            .into_iter()
            .map(|a| {
                let mut p = paths[a.origin].clone();
                p.extend(a.line.into_iter().skip(1));
                p
            })
            .collect();
    }
    let mut out = paths.swap_remove(0);
    out.dedup_by(|a, b| norm(sub(*a, *b)) < 1e-6);
    Ok(out)
}
/// A pose at a leg's starting waypoint: heading, accumulated cost and the route it ends.
struct Start {
    heading: f64,
    g: f64,
    origin: usize,
}
/// One way of finishing a leg, cheapest first.
struct Arrival {
    heading: f64,
    g: f64,
    origin: usize,
    line: Vec<Point>,
}

/// One leg, from `waypoints[stage]` exactly through `waypoints[stage + 1]`. The exact
/// (optimal) search runs first; if it exceeds its budget on a hard leg, weighted searches
/// find a near-optimal route instead. An exact search that runs out of states proves that
/// no route satisfies the constraints.
#[allow(clippy::too_many_arguments)]
fn leg(
    terrain: &Terrain,
    waypoints: &[Point],
    stage: usize,
    starts: &[Start],
    s: &RoutingSettings,
    forbidden: &[Vec<Point>],
    field: &CostField,
    cancel: &AtomicBool,
) -> Result<Vec<Arrival>> {
    let mut closest = f64::INFINITY;
    let length = norm(sub(waypoints[stage + 1], waypoints[stage]));
    for (weight, budget) in [(1., 500_000), (1.5, 1_000_000), (3., 1_500_000)] {
        match leg_search(
            terrain, waypoints, stage, starts, s, forbidden, field, cancel, weight, budget,
        )? {
            Ok(arrivals) => return Ok(arrivals),
            Err((true, near)) => closest = closest.min(near),
            // The road keeps its heading through a waypoint; it could not get going at all.
            Err((false, near)) if stage > 0 && near >= length - 1e-6 => bail!(
                "Droga nie może wyjechać z punktu {} w kierunku, w którym do niego dojeżdża. Przesuń punkt, dodaj punkt pośredni lub złagodź ograniczenia",
                stage + 1
            ),
            Err((false, near)) => bail!(
                "Nie znaleziono trasy spełniającej ograniczenia. Do punktu {} dojechano najbliżej na {:.0} m. Zwiększ spadek maks., zmniejsz promień min. lub krok automatu",
                stage + 2,
                closest.min(near)
            ),
        }
    }
    bail!(
        "Przekroczono budżet wyszukiwania. Do punktu {} dojechano najbliżej na {:.0} m. Zwiększ krok, złagodź ograniczenia lub dodaj punkty pośrednie",
        stage + 2,
        closest
    )
}

/// Hybrid A* for one leg with the heuristic scaled by `weight` and at most `budget` states.
/// Without `starts` the first heading is free. The last leg returns only the best route;
/// earlier legs also keep the cheapest arrival from each direction up to 2× the best cost.
/// When nothing is found, returns whether the budget ran out and the closest approach.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn leg_search(
    terrain: &Terrain,
    waypoints: &[Point],
    stage: usize,
    starts: &[Start],
    s: &RoutingSettings,
    forbidden: &[Vec<Point>],
    field: &CostField,
    cancel: &AtomicBool,
    weight: f64,
    budget: usize,
) -> Result<std::result::Result<Vec<Arrival>, (bool, f64)>> {
    let clearance = s.tolerance;
    let (from, target) = (waypoints[stage], waypoints[stage + 1]);
    let step = s.cell;
    // A step of `cell` metres turning by `max_turn` keeps the radius at or above `min_radius`.
    let max_turn = (step / s.min_radius).min(FRAC_PI_4);
    let turns = [0., -0.5, 0.5, -1., 1.].map(|k| k * max_turn);
    let bins = ((TAU / (max_turn / 4.)).ceil() as usize).clamp(32, 180);
    let resolution = step / 2.;
    let reach = (3. * step).max(s.min_radius.min(300.));
    let target_z = terrain.sample(target).unwrap_or(0.);
    // Lower bound of the leg: never shorter than the distance, nor than the climb divided
    // by the maximum grade (a steep climb needs a longer road of whatever shape).
    let heuristic = |n: &State| {
        if n.stage == 1 {
            return 0.;
        }
        let dz = (target_z - terrain.sample(n.p).unwrap_or(target_z)).abs();
        let analytic = norm(sub(target, n.p)).max(dz / (s.max_grade / 100.)) + s.slope_weight * dz;
        analytic.max(field.estimate(stage, n.p))
    };
    let key = |n: &State| {
        (
            n.stage,
            (n.p[0] / resolution).floor() as i64,
            (n.p[1] / resolution).floor() as i64,
            (n.heading.rem_euclid(TAU) / TAU * bins as f64).round() as usize % bins,
        )
    };
    // A small curvature cost prefers straight runs over needless weaving.
    let edge_cost = |length: f64, climb: f64, delta: f64| {
        length + s.slope_weight * climb + 0.1 * delta.abs() * s.min_radius
    };
    let mut nodes: Vec<State> = Vec::new();
    let mut best: HashMap<(usize, i64, i64, usize), usize> = HashMap::new();
    let mut heap = BinaryHeap::new();
    let mut push = |n: State, nodes: &mut Vec<State>, heap: &mut BinaryHeap<Node>| {
        let k = key(&n);
        if best.get(&k).is_some_and(|&i| nodes[i].g <= n.g + 1e-9) {
            return;
        }
        let index = nodes.len();
        heap.push(Node {
            cost: n.g + weight * heuristic(&n),
            index,
        });
        nodes.push(n);
        best.insert(k, index);
    };
    let toward = sub(target, from);
    let free: Vec<Start>;
    let starts = if starts.is_empty() {
        free = std::iter::once(toward[1].atan2(toward[0]))
            .chain((0..bins).map(|b| b as f64 * TAU / bins as f64))
            .map(|heading| Start { heading, g: 0., origin: 0 })
            .collect();
        &free
    } else {
        starts
    };
    for start in starts {
        push(
            State {
                p: from,
                heading: start.heading,
                stage: 0,
                g: start.g,
                parent: usize::MAX,
                delta: 0.,
                length: 0.,
                origin: start.origin,
            },
            &mut nodes,
            &mut heap,
        );
    }
    let last = stage + 2 == waypoints.len();
    let mut goals: Vec<usize> = Vec::new();
    // States stored when the first arrival was found; bounds the search for alternatives.
    let mut found_at = usize::MAX;
    let mut exhausted = false;
    let mut pops = 0usize;
    while let Some(Node { index, cost }) = heap.pop() {
        pops += 1;
        if pops.is_multiple_of(256) {
            ensure!(!cancel.load(AO::Relaxed), "Wyznaczanie anulowane");
        }
        let n = nodes[index];
        if cost > n.g + weight * heuristic(&n) + 1e-6 {
            continue;
        }
        if let Some(&first) = goals.first()
            && (cost > nodes[first].g * 2. + 100. || nodes.len() > found_at + found_at / 2 + 50_000)
        {
            break;
        }
        if n.stage == 1 {
            // The road keeps its heading through a waypoint, so an intermediate arrival is
            // only useful if the next leg can leave with it. Keep the cheapest such arrival
            // per 45° sector, so the next leg can choose its direction (e.g. near the map edge).
            let departs = last
                || turns.iter().any(|&delta| {
                    profile(&arc(target, n.heading, delta, step), terrain, s, forbidden, clearance)
                        .is_some()
                });
            let sector = |h: f64| (h.rem_euclid(TAU) / TAU * 8.) as usize % 8;
            if departs && !goals.iter().any(|&g| sector(nodes[g].heading) == sector(n.heading)) {
                goals.push(index);
                found_at = found_at.min(nodes.len());
            }
            if last || goals.len() == 8 {
                break;
            }
            continue;
        }
        if nodes.len() > budget {
            exhausted = true;
            break;
        }
        let mut expand = |delta: f64, length: f64, arrived: usize, exact: Option<Point>| {
            let line = arc(n.p, n.heading, delta, length);
            if let Some(climb) = profile(&line, terrain, s, forbidden, clearance) {
                push(
                    State {
                        p: exact.unwrap_or(*line.last().unwrap()),
                        heading: n.heading + delta,
                        stage: arrived,
                        g: n.g + edge_cost(length, climb, delta),
                        parent: index,
                        delta,
                        length,
                        origin: n.origin,
                    },
                    &mut nodes,
                    &mut heap,
                );
            }
        };
        if norm(sub(target, n.p)) <= reach
            && let Some((delta, length)) = connect(n.p, n.heading, target, s.min_radius)
        {
            expand(delta, length, 1, Some(target));
        }
        for delta in turns {
            expand(delta, step, 0, None);
        }
    }
    ensure!(!cancel.load(AO::Relaxed), "Wyznaczanie anulowane");
    if goals.is_empty() {
        // Report how far the road got, so the user knows which leg needs other settings.
        let closest = nodes
            .iter()
            .map(|n| norm(sub(n.p, target)))
            .fold(f64::INFINITY, f64::min);
        return Ok(Err((exhausted, closest)));
    }
    Ok(Ok(goals
        .into_iter()
        .map(|goal| {
            let mut chain = Vec::new();
            let mut at = goal;
            while at != usize::MAX {
                chain.push(at);
                at = nodes[at].parent;
            }
            chain.reverse();
            let mut line = vec![from];
            for pair in chain.windows(2) {
                let (a, b) = (nodes[pair[0]], nodes[pair[1]]);
                let mut part = arc(a.p, a.heading, b.delta, b.length);
                *part.last_mut().unwrap() = b.p;
                line.extend(part.into_iter().skip(1));
            }
            let n = nodes[goal];
            Arrival { heading: n.heading, g: n.g, origin: nodes[chain[0]].origin, line }
        })
        .collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_around_polygon_and_rejects_grade_and_nodata() {
        let dir = std::env::temp_dir().join(format!("road-routing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("flat.asc");
        let mut text = "ncols 21\nnrows 21\nxllcorner 0\nyllcorner 0\ncellsize 10\n".to_owned();
        for _ in 0..441 {
            text.push_str("100 ");
        }
        std::fs::write(&source, text).unwrap();
        let cancel = AtomicBool::new(false);
        let terrain = crate::terrain::import(&source, &dir, &cancel).unwrap();
        let forbidden = vec![vec![[85., 60.], [125., 60.], [125., 150.], [85., 150.]]];
        let settings = RoutingSettings {
            cell: 10.,
            ..Default::default()
        };
        let path = find(
            &terrain,
            &[[25., 105.], [185., 105.]],
            &settings,
            &forbidden,
            &cancel,
        )
        .unwrap();
        assert!(path.len() > 2);
        assert_eq!(path[0], [25., 105.]);
        assert_eq!(*path.last().unwrap(), [185., 105.]);
        assert!(
            path.windows(2)
                .all(|w| permitted(w[0], w[1], &terrain, &settings, &forbidden))
        );
        assert!(blocked_segment([0., 100.], [200., 100.], &forbidden));
        assert!(!blocked_segment([0., 20.], [200., 20.], &forbidden));
        cancel.store(true, AO::Relaxed);
        assert!(
            find(
                &terrain,
                &[[25., 105.], [185., 105.]],
                &settings,
                &[],
                &cancel
            )
            .is_err()
        );
        drop(terrain);
        let mut text =
            "ncols 3\nnrows 2\nxllcorner 0\nyllcorner 0\ncellsize 10\nNODATA_value -9999\n"
                .to_owned();
        text.push_str("0 100 -9999 0 100 -9999");
        std::fs::write(&source, text).unwrap();
        cancel.store(false, AO::Relaxed);
        let t = crate::terrain::import(&source, &dir, &cancel).unwrap();
        assert!(!permitted([5., 5.], [15., 5.], &t, &settings, &[]));
        assert!(find(&t, &[[5., 5.], [25., 5.]], &settings, &[], &cancel).is_err());
        drop(t);
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 610 × 610 m ASC (10 m cells) with a Gaussian hill (`height` m, `sigma` m) at (300, 300).
    fn hill(name: &str, height: f64, sigma: f64) -> (std::path::PathBuf, Terrain) {
        let dir = std::env::temp_dir().join(format!("road-hill-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = "ncols 61\nnrows 61\nxllcorner 0\nyllcorner 0\ncellsize 10\n".to_owned();
        for row in 0..61 {
            for col in 0..61 {
                let (x, y) = (col as f64 * 10. + 5., (60 - row) as f64 * 10. + 5.);
                let d2 = (x - 300.).powi(2) + (y - 300.).powi(2);
                text.push_str(&format!("{:.4} ", 100. + height * (-d2 / (2. * sigma * sigma)).exp()));
            }
            text.push('\n');
        }
        let source = dir.join("hill.asc");
        std::fs::write(&source, text).unwrap();
        let terrain = crate::terrain::import(&source, &dir, &AtomicBool::new(false)).unwrap();
        (dir, terrain)
    }
    fn heading(a: Point, b: Point) -> f64 {
        (b[1] - a[1]).atan2(b[0] - a[0])
    }
    fn turn(a: f64, b: f64) -> f64 {
        (b - a + std::f64::consts::PI).rem_euclid(TAU) - std::f64::consts::PI
    }
    /// Smallest radius implied by consecutive segments of the route.
    fn tightest_radius(path: &[Point]) -> f64 {
        path.windows(3)
            .map(|w| {
                let t = turn(heading(w[0], w[1]), heading(w[1], w[2])).abs();
                let l = norm(sub(w[1], w[0])).min(norm(sub(w[2], w[1])));
                if t < 1e-9 { f64::INFINITY } else { l / t }
            })
            .fold(f64::INFINITY, f64::min)
    }
    fn climb(path: &[Point], t: &Terrain) -> f64 {
        profile(path, t, &RoutingSettings { max_grade: 1e9, ..Default::default() }, &[], 0.).unwrap()
    }

    #[test]
    fn terrain_route_respects_grade_radius_and_passes_every_waypoint() {
        let (dir, t) = hill("waypoints", 80., 60.);
        let cancel = AtomicBool::new(false);
        let s = RoutingSettings::default();
        let waypoints = [[40., 300.], [300., 80.], [560., 300.]];
        let path = find(&t, &waypoints, &s, &[], &cancel).unwrap();
        for w in waypoints {
            assert!(path.iter().any(|p| norm(sub(*p, w)) < 1e-6), "route misses {w:?}");
        }
        assert_eq!(path[0], waypoints[0]);
        assert_eq!(*path.last().unwrap(), waypoints[2]);
        assert!(path.windows(2).all(|w| permitted(w[0], w[1], &t, &s, &[])));
        // Sampled arcs: allow the chord approximation of the exact radius.
        assert!(tightest_radius(&path) >= s.min_radius * 0.98, "{}", tightest_radius(&path));
        // The heading stays continuous through the intermediate waypoint.
        let i = path.iter().position(|p| norm(sub(*p, waypoints[1])) < 1e-6).unwrap();
        let kink = turn(heading(path[i - 1], path[i]), heading(path[i], path[i + 1])).abs();
        assert!(kink < 3f64.to_radians(), "{kink}");
        // Straight over the summit would exceed 12 %; the route detours.
        assert!(!permitted(waypoints[0], waypoints[2], &t, &s, &[]));
        let direct = find(&t, &[waypoints[0], waypoints[2]], &s, &[], &cancel).unwrap();
        assert!(direct.windows(2).all(|w| permitted(w[0], w[1], &t, &s, &[])));
        assert!(direct.iter().all(|p| norm(sub(*p, [300., 300.])) > 100.));

        let wide = RoutingSettings { min_radius: 120., ..Default::default() };
        let path = find(&t, &waypoints, &wide, &[], &cancel).unwrap();
        assert!(tightest_radius(&path) >= wide.min_radius * 0.98);
        drop(t);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn steep_climb_to_summit_lengthens_the_road_within_max_grade() {
        // 50 m hill with slopes up to ~28 %: A at the foot, B on the summit. The shape of
        // the climb (spiral, serpentines, detour) is left to the search.
        let (dir, t) = hill("summit", 50., 110.);
        let cancel = AtomicBool::new(false);
        let (a, b) = ([300., 30.], [300., 300.]);
        let mut lengths = Vec::new();
        for (max_grade, min_radius) in [(12., 25.), (15., 15.), (20., 40.)] {
            let s = RoutingSettings { max_grade, min_radius, cell: 10., ..Default::default() };
            assert!(!permitted(a, b, &t, &s, &[]));
            let path = find(&t, &[a, b], &s, &[], &cancel).unwrap();
            assert_eq!((path[0], *path.last().unwrap()), (a, b));
            assert!(path.windows(2).all(|w| permitted(w[0], w[1], &t, &s, &[])));
            assert!(tightest_radius(&path) >= min_radius * 0.98);
            let length: f64 = path.windows(2).map(|w| norm(sub(w[1], w[0]))).sum();
            let rise = t.sample(b).unwrap() - t.sample(a).unwrap();
            assert!(length >= rise / (max_grade / 100.), "{length}");
            lengths.push(length);
        }
        // A stricter grade needs a longer road; a gentle one may stay nearly straight.
        assert!(lengths[0] > lengths[2] * 1.2, "{lengths:?}");
        // Too strict for any road: the failure names the leg and the closest approach.
        let s = RoutingSettings { max_grade: 2., min_radius: 200., cell: 10., ..Default::default() };
        let err = find(&t, &[a, b], &s, &[], &cancel).unwrap_err().to_string();
        assert!(err.contains("Do punktu 2 dojechano najbliżej"), "{err}");
        drop(t);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn slope_weight_trades_length_for_climb_and_tolerance_keeps_clearance() {
        let (dir, t) = hill("weights", 20., 60.);
        let cancel = AtomicBool::new(false);
        let waypoints = [[40., 300.], [560., 300.]];
        let flat = RoutingSettings { max_grade: 100., slope_weight: 0., ..Default::default() };
        let steep = RoutingSettings { slope_weight: 30., ..flat.clone() };
        let short = find(&t, &waypoints, &flat, &[], &cancel).unwrap();
        let gentle = find(&t, &waypoints, &steep, &[], &cancel).unwrap();
        let length = |p: &[Point]| p.windows(2).map(|w| norm(sub(w[1], w[0]))).sum::<f64>();
        assert!(length(&short) < 525.);
        assert!(climb(&gentle, &t) < climb(&short, &t) * 0.6);
        assert!(length(&gentle) > length(&short));

        let zone = vec![vec![[280., 200.], [320., 200.], [320., 400.], [280., 400.]]];
        for tolerance in [2., 15.] {
            let s = RoutingSettings { tolerance, ..flat.clone() };
            let path = find(&t, &waypoints, &s, &zone, &cancel).unwrap();
            let gap = path
                .windows(2)
                .map(|w| polygon_distance(w[0], w[1], &zone[0]))
                .fold(f64::INFINITY, f64::min);
            assert!(gap >= tolerance - 1e-6, "{gap} < {tolerance}");
        }
        let s = RoutingSettings { tolerance: 30., ..flat.clone() };
        assert!(find(&t, &[[255., 300.], [560., 300.]], &s, &zone, &cancel).is_err());
        drop(t);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn terrain_route_is_fitted_with_models_through_waypoints() {
        use crate::roads::{CatalogPart, centerline, endpoint};
        let catalog: Vec<_> = [
            ("asf2_6.p3d", 3),
            ("asf2_12.p3d", 3),
            ("asf2_25.p3d", 3),
            ("asf2_10 25.p3d", 4),
            ("asf2_10 50.p3d", 4),
            ("asf2_10 100.p3d", 4),
            ("asf2_6konec.p3d", 6),
        ]
        .into_iter()
        .map(|(name, category)| CatalogPart {
            path: name.into(),
            family: "asf2".into(),
            category,
            index: 0,
            road_type_index: 0,
            model: crate::geometry::filename_model(name).unwrap(),
        })
        .collect();
        let cancel = AtomicBool::new(false);
        // Around a hill through an intermediate point, and from its foot up to the summit.
        for (name, height, sigma, cell, max_grade, waypoints) in [
            ("fit", 80., 60., 20., 12., vec![[40., 300.], [300., 80.], [560., 300.]]),
            ("fit-summit", 50., 110., 10., 15., vec![[300., 30.], [300., 300.]]),
        ] {
            let (dir, t) = hill(name, height, sigma);
            let s = RoutingSettings { cell, max_grade, ..Default::default() };
            let parts = plan(&t, &waypoints, "asf2", &catalog, &s, &[], &cancel).unwrap();
            let road = centerline(&parts, &catalog);
            let end = *waypoints.last().unwrap();
            assert!(norm(sub(endpoint(&parts, &catalog).unwrap().p, end)) <= s.tolerance);
            for w in &waypoints {
                let miss = road.iter().map(|p| norm(sub(*p, *w))).fold(f64::INFINITY, f64::min);
                assert!(miss <= s.tolerance, "{w:?} missed by {miss}");
            }
            // The fitted models themselves respect the maximum grade.
            assert!(road.windows(2).all(|w| permitted(w[0], w[1], &t, &s, &[])));
            drop(t);
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
}
