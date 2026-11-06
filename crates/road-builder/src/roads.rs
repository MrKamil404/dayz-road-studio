use crate::{
    document::RoutingSettings,
    geometry::{Library, Model, Point, Port, Shape, add, mul, norm, rotate, sub},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct PlacedPart {
    pub model: String,
    pub reverse: bool,
    pub position: Point,
    pub rotation: f64,
}
#[derive(Clone)]
pub struct CatalogPart {
    pub path: String,
    pub family: String,
    pub category: u32,
    pub index: u32,
    /// Position of the parent definition in TV4P's 0x88 list (not the part index).
    pub road_type_index: u32,
    pub model: Model,
}
pub fn append(
    parts: &mut Vec<PlacedPart>,
    catalog: &[CatalogPart],
    name: &str,
    reverse: bool,
    start: Point,
    angle: f64,
) -> Result<()> {
    let item = catalog
        .iter()
        .find(|p| p.path == name)
        .context("Brak modelu w katalogu")?;
    let at = if parts.is_empty() {
        Port {
            p: start,
            outward: [angle.cos(), angle.sin()],
        }
    } else {
        endpoint(parts, catalog)?
    };
    let p = place(item, at, reverse)?;
    parts.push(p);
    Ok(())
}
pub fn place(item: &CatalogPart, at: Port, reverse: bool) -> Result<PlacedPart> {
    let port = item.model.ports[usize::from(reverse)].context("Brak portu modelu")?;
    let target = mul(at.outward, -1.);
    let angle = target[1].atan2(target[0]) - port.outward[1].atan2(port.outward[0]);
    Ok(PlacedPart {
        model: item.path.clone(),
        reverse,
        position: sub(at.p, rotate(port.p, angle)),
        rotation: -angle.to_degrees(),
    })
}
pub fn endpoint(parts: &[PlacedPart], catalog: &[CatalogPart]) -> Result<Port> {
    let p = parts.last().context("Pusta droga")?;
    let m = &catalog
        .iter()
        .find(|m| m.path == p.model)
        .context("Brak modelu")?
        .model;
    placed_endpoint(p, m)
}
fn placed_endpoint(p: &PlacedPart, m: &Model) -> Result<Port> {
    let port = m.ports[1 - usize::from(p.reverse)].context("Brak wyjścia modelu")?;
    let angle = -p.rotation.to_radians();
    Ok(Port {
        p: add(rotate(port.p, angle), p.position),
        outward: rotate(port.outward, angle),
    })
}
pub fn shape(parts: &[PlacedPart], lib: &mut Library) -> Result<Shape> {
    let mut s = Shape::default();
    for p in parts {
        let m = lib.model(&p.model)?;
        s.place(&m, -p.rotation.to_radians(), p.position);
    }
    Ok(s)
}
pub fn centerline(parts: &[PlacedPart], catalog: &[CatalogPart]) -> Vec<Point> {
    let mut out = Vec::new();
    for p in parts {
        if let Some(item) = catalog.iter().find(|m| m.path == p.model) {
            let mut line = item.model.line.clone();
            if p.reverse {
                line.reverse();
            }
            for q in line {
                let world = add(rotate(q, -p.rotation.to_radians()), p.position);
                if out.last().is_none_or(|v| norm(sub(*v, world)) > 1e-6) {
                    out.push(world);
                }
            }
        }
    }
    out
}
/// Round polyline corners by a circular fillet; never shrink the requested radius.
pub fn smooth(points: &[Point], radius: f64) -> Result<Vec<Point>> {
    ensure!(
        points.len() >= 2 && radius.is_finite() && radius > 0.,
        "Nieprawidłowa trasa lub promień"
    );
    let mut out = vec![points[0]];
    for w in points.windows(3) {
        let incoming = sub(w[1], w[0]);
        let outgoing = sub(w[2], w[1]);
        let a = norm(incoming);
        let b = norm(outgoing);
        ensure!(a > 1e-6 && b > 1e-6, "Usuń powtórzone punkty trasy");
        let u = mul(incoming, 1. / a);
        let v = mul(outgoing, 1. / b);
        let dot = (u[0] * v[0] + u[1] * v[1]).clamp(-1., 1.);
        let turn = dot.acos();
        if turn < 0.01 {
            out.push(w[1]);
            continue;
        }
        ensure!(
            turn < std::f64::consts::PI - 0.01,
            "Trasa zawraca w jednym punkcie"
        );
        let tangent = radius * (turn / 2.).tan();
        ensure!(
            tangent <= a * 0.45 && tangent <= b * 0.45,
            "Punkty są za blisko, aby zmieścić promień {radius} m. Rozsuń punkty lub zmniejsz promień"
        );
        let sign = (u[0] * v[1] - u[1] * v[0]).signum();
        let entry = sub(w[1], mul(u, tangent));
        let center = add(entry, mul([-u[1], u[0]], radius * sign));
        let radial = sub(entry, center);
        let steps = (turn.to_degrees() / 2.).ceil().max(2.) as usize;
        for i in 0..=steps {
            out.push(add(
                center,
                rotate(radial, turn * sign * i as f64 / steps as f64),
            ));
        }
    }
    out.push(*points.last().unwrap());
    Ok(out)
}
/// Nearest point of `line` limited to the progress window `[lo, hi]`, as (distance, progress).
/// The window keeps parallel legs of serpentines from being mistaken for the current one.
fn nearest_progress_in(p: Point, line: &[Point], lo: f64, hi: f64) -> (f64, f64) {
    let mut walked = 0.;
    let mut best = (f64::INFINITY, 0.);
    for w in line.windows(2) {
        let delta = sub(w[1], w[0]);
        let length = norm(delta);
        if length < 1e-8 {
            continue;
        }
        let (t0, t1) = (
            ((lo - walked) / length).max(0.),
            ((hi - walked) / length).min(1.),
        );
        if t0 > t1 {
            walked += length;
            continue;
        }
        let rel = sub(p, w[0]);
        let t = ((rel[0] * delta[0] + rel[1] * delta[1]) / (length * length)).clamp(t0, t1);
        let d = norm(sub(p, add(w[0], mul(delta, t))));
        if d < best.0 {
            best = (d, walked + t * length);
        }
        walked += length;
    }
    best
}
fn direction_at(line: &[Point], progress: f64) -> Point {
    let mut walked = 0.;
    let mut direction = [0., 1.];
    for w in line.windows(2) {
        let delta = sub(w[1], w[0]);
        let length = norm(delta);
        if length < 1e-8 {
            continue;
        }
        direction = mul(delta, 1. / length);
        if walked + length > progress {
            return direction;
        }
        walked += length;
    }
    direction
}
/// Greedy connector-exact fitting with lookahead. Failure leaves the document untouched.
pub fn fit(
    points: &[Point],
    family: &str,
    catalog: &[CatalogPart],
    s: &RoutingSettings,
    allowed: impl Fn(Point, Point) -> bool,
) -> Result<Vec<PlacedPart>> {
    ensure!(points.len() >= 2, "Wskaż przynajmniej dwa punkty");
    let smoothed = smooth(points, s.min_radius)?;
    fit_path(&smoothed, family, catalog, s, allowed)
}
/// Fit models to an already curvature-limited centreline (for example a terrain route)
/// without rounding its corners again, so the road stays on every point of the line.
pub fn fit_path(
    points: &[Point],
    family: &str,
    catalog: &[CatalogPart],
    s: &RoutingSettings,
    allowed: impl Fn(Point, Point) -> bool,
) -> Result<Vec<PlacedPart>> {
    ensure!(points.len() >= 2, "Wskaż przynajmniej dwa punkty");
    let cap=catalog.iter().filter(|m|m.family==family && m.category==6)
        .find(|m|m.path.rsplit(['\\','/']).next().unwrap_or("").to_lowercase().contains("konec"))
        .context("Brak segmentu konec MLOD dla wybranego typu drogi. Dodaj go do definicji Road Tool i wczytaj modele")?;
    ensure!(
        cap.model.ports[0].is_some() && cap.model.ports[1].is_some(),
        "Segment konec nie ma obu portów połączenia"
    );
    let choices: Vec<_> = catalog
        .iter()
        .filter(|m| {
            m.family == family
                && m.road_type_index == cap.road_type_index
                && [3, 4].contains(&m.category)
                && m.model.ports[2].is_none()
                && m.model.ports[3].is_none()
        })
        .collect();
    ensure!(
        !choices.is_empty(),
        "Brak prostych i zakrętów MLOD dla tej rodziny"
    );
    let length: f64 = points.windows(2).map(|w| norm(sub(w[1], w[0]))).sum();
    let end = *points.last().unwrap();
    let initial = sub(points[1], points[0]);
    ensure!(
        length >= 2. * cap.model.length && norm(initial) > 1e-6,
        "Trasa jest za krótka na dwa segmenty konec lub ma powtórzone punkty"
    );
    let mut at = Port {
        p: points[0],
        outward: mul(initial, 1. / norm(initial)),
    };
    // Search only the stretch of line a single model can cover from the current progress.
    let ahead = choices
        .iter()
        .map(|m| m.model.length)
        .fold(cap.model.length, f64::max)
        + s.tolerance * 2.
        + 10.;
    let near = |p: Point, from: f64| nearest_progress_in(p, points, from - s.tolerance * 2., from + ahead);
    let valid_part = |part: &PlacedPart, model: &Model, from: f64| {
        let world: Vec<_> = model
            .line
            .iter()
            .map(|q| add(rotate(*q, -part.rotation.to_radians()), part.position))
            .collect();
        world.len() >= 2
            && world.iter().all(|q| near(*q, from).0 <= s.tolerance)
            && world.windows(2).all(|w| allowed(w[0], w[1]))
    };
    let first = place(cap, at, false)?;
    ensure!(
        valid_part(&first, &cap.model, 0.),
        "Początkowy segment konec nie mieści się na trasie lub nie spełnia ograniczeń terenu"
    );
    at = placed_endpoint(&first, &cap.model)?;
    let mut progress = near(at.p, 0.).1;
    let mut parts = vec![first];
    // Depth-first with the best-scored models first: when no model fits, step back and try
    // the next alternative of an earlier choice. Frame `i` holds the untried alternatives
    // for `parts[i + 1]`, best last.
    let mut stack: Vec<Vec<(PlacedPart, Port, f64)>> = Vec::new();
    let mut furthest = progress;
    for _ in 0..50000 {
        let last = place(cap, at, false)?;
        let last_end = placed_endpoint(&last, &cap.model)?;
        if norm(sub(last_end.p, end)) <= s.tolerance
            && near(last_end.p, progress).1 >= length - s.tolerance * 2.
            && valid_part(&last, &cap.model, progress)
        {
            parts.push(last);
            return Ok(parts);
        }
        let mut options = Vec::new();
        for item in &choices {
            for reverse in [false, true] {
                // Only curves support the native reversed-segment code.
                if reverse && item.category != 4 {
                    continue;
                }
                let model = &item.model;
                let a = model.ports[0].context("Brak początku")?;
                let b = model.ports[1].context("Brak końca")?;
                let turn = (-a.outward[0] * b.outward[1] + a.outward[1] * b.outward[0])
                    .atan2(-a.outward[0] * b.outward[0] - a.outward[1] * b.outward[1])
                    .abs();
                if turn > 1e-5 && model.length / turn < s.min_radius - 1e-6 {
                    continue;
                }
                let p = place(item, at, reverse)?;
                let angle = -p.rotation.to_radians();
                let exit = model.ports[1 - usize::from(reverse)].unwrap();
                let next = Port {
                    p: add(rotate(exit.p, angle), p.position),
                    outward: rotate(exit.outward, angle),
                };
                let (deviation, pr) = near(next.p, progress);
                if pr <= progress + 0.01 || deviation > s.tolerance
                    || pr + cap.model.length > length + s.tolerance
                {
                    continue;
                }
                let mut line = model.line.clone();
                if reverse {
                    line.reverse();
                }
                let world: Vec<_> = line
                    .iter()
                    .map(|q| add(rotate(*q, angle), p.position))
                    .collect();
                if world
                    .iter()
                    .any(|q| near(*q, progress).0 > s.tolerance)
                    || !world.windows(2).all(|w| allowed(w[0], w[1]))
                {
                    continue;
                }
                let remaining = length - pr;
                // Looking past the final point makes a straight exit appear worse
                // than a curve that curls back into the route's end corridor.
                let look = add(next.p, mul(next.outward, remaining.clamp(0., 10.)));
                let future = near(look, progress).0;
                let target_direction = direction_at(points, pr);
                let heading_error = (next.outward[0] * target_direction[1]
                    - next.outward[1] * target_direction[0])
                    .atan2(next.outward[0] * target_direction[0]
                        + next.outward[1] * target_direction[1])
                    .abs();
                let incoming_error = (at.outward[0] * target_direction[1]
                    - at.outward[1] * target_direction[0])
                    .atan2(at.outward[0] * target_direction[0]
                        + at.outward[1] * target_direction[1])
                    .abs();
                if turn > 1e-5 && heading_error >= incoming_error
                    && near(at.p, progress).0 < s.tolerance * 0.5
                {
                    continue;
                }
                // Endpoint distance alone rewards alternating curves inside the
                // tolerance corridor. Prefer aligned exits and avoid needless turns.
                let score = deviation * 3.
                    + future
                    + heading_error * 4.
                    + turn
                    + if remaining < 50. {
                        norm(sub(next.p, end)) * 0.2
                    } else {
                        0.
                    }
                    - model.length * 0.03;
                options.push((score, p, next, pr));
            }
        }
        if !options.is_empty() {
            options.sort_by(|a, b| b.0.total_cmp(&a.0));
            let keep = options.len().saturating_sub(4);
            stack.push(
                options
                    .into_iter()
                    .skip(keep)
                    .map(|(_, p, next, pr)| (p, next, pr))
                    .collect(),
            );
        }
        loop {
            let Some(frame) = stack.last_mut() else {
                anyhow::bail!(
                    "Nie można dopasować modeli przy {:.0} m trasy. Złagodź zakręty, zwiększ tolerancję lub użyj segmentów",
                    furthest
                );
            };
            if let Some((p, next, pr)) = frame.pop() {
                parts.truncate(stack.len());
                parts.push(p);
                at = next;
                progress = pr;
                furthest = furthest.max(pr);
                break;
            }
            stack.pop();
        }
    }
    anyhow::bail!("Przekroczono limit prób dopasowania modeli. Złagodź zakręty, zwiększ tolerancję lub dodaj punkty pośrednie")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearly_straight_routes_do_not_weave_with_available_curves() {
        let c: Vec<_> = [
            ("asf2_6.p3d", 3), ("asf2_12.p3d", 3), ("asf2_25.p3d", 3),
            ("asf2_10 25.p3d", 4), ("asf2_10 100.p3d", 4),
            ("asf2_6konec.p3d", 6),
        ].into_iter().map(|(name, category)| CatalogPart {
            path: name.into(), family: "asf2".into(), category,
            index: 0, road_type_index: 0,
            model: crate::geometry::filename_model(name).unwrap(),
        }).collect();
        for points in [
            vec![[0., 0.], [0., 200.]],
            vec![[0., 0.], [0., 100.], [1., 200.]],
            vec![[0., 0.], [0., 100.], [2., 200.]],
        ] {
            let settings = RoutingSettings::default();
            let parts = fit(&points, "asf2", &c, &settings, |_, _| true).unwrap();
            assert!(parts.iter().all(|p| !p.model.contains(' ')), "{parts:?}");
            assert!(norm(sub(endpoint(&parts, &c).unwrap().p, *points.last().unwrap())) <= settings.tolerance);
        }
    }
    #[test]
    fn rounded_corner_is_fitted_with_available_curves() {
        let mut c = Vec::new();
        for (name, category) in [
            ("asf2_6.p3d", 3),
            ("asf2_12.p3d", 3),
            ("asf2_25.p3d", 3),
            ("asf2_10 25.p3d", 4),
            ("asf2_30 25.p3d", 4),
            ("asf2_6konec.p3d", 6),
        ] {
            c.push(CatalogPart {
                path: name.into(),
                family: "asf2".into(),
                category,
                index: 0,
                road_type_index: 0,
                model: crate::geometry::filename_model(name).unwrap(),
            });
        }
        let settings = RoutingSettings::default();
        let parts = fit(
            &[[0., 0.], [0., 100.], [100., 100.]],
            "asf2",
            &c,
            &settings,
            |_, _| true,
        )
        .unwrap();
        assert!(
            parts
                .iter()
                .any(|p| p.model.contains("25.p3d") && p.model.contains(' '))
        );
        assert!(norm(sub(endpoint(&parts, &c).unwrap().p, [100., 100.])) <= settings.tolerance);
    }
    #[test]
    fn fit_and_manual_connectors_match() {
        let mut c = vec![CatalogPart {
            path: "asf2_25.p3d".into(),
            family: "asf2".into(),
            category: 3,
            index: 0,
            road_type_index: 0,
            model: crate::geometry::filename_model("asf2_25.p3d").unwrap(),
        }];
        c.push(CatalogPart {
            path: "asf2_6konec.p3d".into(),
            family: "asf2".into(),
            category: 6,
            index: 0,
            road_type_index: 0,
            model: crate::geometry::filename_model("asf2_6konec.p3d").unwrap(),
        });
        let p = fit(
            &[[200000., 0.], [200000., 112.5]],
            "asf2",
            &c,
            &RoutingSettings::default(),
            |_, _| true,
        )
        .unwrap();
        assert_eq!(p.len(), 6);
        assert_eq!(p.first().unwrap().model, "asf2_6konec.p3d");
        assert_eq!(p.last().unwrap().model, "asf2_6konec.p3d");
        assert_eq!(endpoint(&p, &c).unwrap().p, [200000., 112.5]);
        for i in 1..p.len() {
            let at = endpoint(&p[..i], &c).unwrap();
            let item = c.iter().find(|m| m.path == p[i].model).unwrap();
            let expected = place(item, at, p[i].reverse).unwrap();
            assert!(norm(sub(expected.position, p[i].position)) < 1e-8);
        }
        assert!(
            fit(
                &[[0., 0.], [0., 5.]],
                "asf2",
                &c,
                &RoutingSettings::default(),
                |_, _| true
            )
            .is_err()
        );
        assert!(
            fit(
                &[[0., 0.], [0., 112.5]],
                "asf2",
                &c[..1],
                &RoutingSettings::default(),
                |_, _| true
            )
            .is_err()
        );
        assert!(
            fit(
                &[[0., 0.], [0., 112.5]],
                "asf2",
                &c,
                &RoutingSettings::default(),
                |a, _| a[1] >= 7.
            )
            .is_err()
        );
        assert!(
            fit(
                &[[0., 0.], [100., 100.], [0., 100.]],
                "asf2",
                &c,
                &RoutingSettings::default(),
                |_, _| true
            )
            .is_err()
        );
    }
}
