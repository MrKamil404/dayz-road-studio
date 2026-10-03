use crate::{
    document::RoutingSettings,
    geometry::{Point, norm, sub},
    terrain::Terrain,
};
use anyhow::{Result, bail, ensure};
use std::{
    cmp::Ordering,
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
pub fn permitted(
    a: Point,
    b: Point,
    terrain: &Terrain,
    settings: &RoutingSettings,
    forbidden: &[Vec<Point>],
) -> bool {
    if blocked_segment(a, b, forbidden) {
        return false;
    }
    let dist = norm(sub(b, a));
    let steps = (dist / (terrain.meta.cell / 2.).clamp(0.1, 5.))
        .ceil()
        .max(1.) as usize;
    let mut last: Option<f64> = None;
    for i in 0..=steps {
        let f = i as f64 / steps as f64;
        let p = [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        if forbidden.iter().any(|poly| inside(p, poly)) {
            return false;
        }
        let Some(z) = terrain.sample(p) else {
            return false;
        };
        if let Some(old) = last
            && dist > 1e-8
            && (z - old).abs() / (dist / steps as f64) * 100. > settings.max_grade + 1e-6
        {
            return false;
        }
        last = Some(z);
    }
    true
}
#[derive(Clone, Copy)]
struct Node {
    cost: f64,
    key: (i32, i32),
}
impl PartialEq for Node {
    fn eq(&self, o: &Self) -> bool {
        self.cost == o.cost && self.key == o.key
    }
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, o: &Self) -> Ordering {
        o.cost.total_cmp(&self.cost).then(self.key.cmp(&o.key))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

pub fn find(
    terrain: &Terrain,
    waypoints: &[Point],
    s: &RoutingSettings,
    forbidden: &[Vec<Point>],
    cancel: &AtomicBool,
) -> Result<Vec<Point>> {
    ensure!(waypoints.len() >= 2, "Wskaż przynajmniej dwa punkty");
    ensure!(
        s.cell.is_finite() && s.cell > 0. && s.max_grade > 0.,
        "Nieprawidłowe ustawienia automatu"
    );
    let m = &terrain.meta;
    let pos = |k: (i32, i32)| {
        [
            m.east + (k.0 as f64 + 0.5) * s.cell,
            m.north + (k.1 as f64 + 0.5) * s.cell,
        ]
    };
    let key = |p: Point| {
        (
            ((p[0] - m.east) / s.cell - 0.5).round() as i32,
            ((p[1] - m.north) / s.cell - 0.5).round() as i32,
        )
    };
    let mut out = Vec::new();
    for pair in waypoints.windows(2) {
        ensure!(
            terrain.sample(pair[0]).is_some() && terrain.sample(pair[1]).is_some(),
            "Punkty poza ASC lub w NODATA"
        );
        let start = key(pair[0]);
        let end = key(pair[1]);
        ensure!(
            permitted(pair[0], pos(start), terrain, s, forbidden)
                && permitted(pos(end), pair[1], terrain, s, forbidden),
            "Brak dojazdu do punktu przy tej rozdzielczości. Zmniejsz krok automatu"
        );
        let mut heap = BinaryHeap::new();
        let mut costs = HashMap::new();
        let mut previous = HashMap::new();
        costs.insert(start, 0.);
        heap.push(Node {
            cost: norm(sub(pos(start), pair[1])),
            key: start,
        });
        let mut reached = false;
        while let Some(n) = heap.pop() {
            ensure!(!cancel.load(AO::Relaxed), "Wyznaczanie anulowane");
            let g = costs[&n.key];
            if n.cost > g + norm(sub(pos(n.key), pair[1])) + 1e-6 {
                continue;
            }
            if n.key == end {
                reached = true;
                break;
            }
            if costs.len() > 1_000_000 {
                bail!("Przekroczono budżet wyszukiwania. Zwiększ krok lub dodaj punkty pośrednie");
            }
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let k = (n.key.0 + dx, n.key.1 + dy);
                    let a = pos(n.key);
                    let b = pos(k);
                    if !permitted(a, b, terrain, s, forbidden) {
                        continue;
                    }
                    let distance = norm(sub(b, a));
                    let grade =
                        (terrain.sample(b).unwrap() - terrain.sample(a).unwrap()).abs() / distance;
                    let ng = g + distance * (1. + s.slope_weight * grade);
                    if ng < *costs.get(&k).unwrap_or(&f64::INFINITY) {
                        costs.insert(k, ng);
                        previous.insert(k, n.key);
                        heap.push(Node {
                            cost: ng + norm(sub(b, pair[1])),
                            key: k,
                        });
                    }
                }
            }
        }
        ensure!(reached, "Nie znaleziono trasy spełniającej ograniczenia");
        let mut path = vec![pair[1], pos(end)];
        let mut at = end;
        while at != start {
            at = previous[&at];
            path.push(pos(at));
        }
        path.push(pair[0]);
        path.reverse();
        // Shortcut only when the complete segment remains permitted.
        let mut simple = vec![path[0]];
        let mut i = 0;
        while i + 1 < path.len() {
            let mut j = (i + 64).min(path.len() - 1);
            while j > i + 1 && !permitted(path[i], path[j], terrain, s, forbidden) {
                j -= 1;
            }
            simple.push(path[j]);
            i = j;
        }
        if !out.is_empty() {
            simple.remove(0);
        }
        out.extend(simple);
    }
    out.dedup_by(|a, b| norm(sub(*a, *b)) < 1e-8);
    Ok(out)
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
}
