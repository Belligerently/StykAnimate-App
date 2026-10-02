//! IK across joined figures (src/ik-joined.mjs).
//!
//! A join aliases a child's origin to a vertex of its parent, so a chain may
//! run through several figures. Each connected assembly that has a pinned
//! controller is compiled into one temporary skeleton, solved once, and the
//! derived poses are scattered back to the figures' own slots. Nothing of the
//! temporary skeleton outlives the scene build.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::doc::{plan_figure, Figure, Object, Polyfill, Segment};
use crate::eval::Frame;
use crate::rig::{Rig, Spatial, Weighted};

use super::object::{solve_object, Outcome, Pose, Skeleton, Solution};
use super::rig::{plain_plan, RigGroup, RigSources};
use super::{has_pinned, Controller, Table};

/// `ikComponents(frame)`: object slots grouped by the joins that connect them,
/// each group in breadth-first order from its lowest slot.
pub fn components(frame: &Frame) -> Vec<Vec<usize>> {
    let count = frame.objects.len();
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (slot, object) in frame.objects.iter().enumerate() {
        if let Some(join) = object.join
            && (join.slot as usize) < count
        {
            neighbors[slot].push(join.slot as usize);
            neighbors[join.slot as usize].push(slot);
        }
    }
    let mut seen = vec![false; count];
    let mut groups = Vec::new();
    for slot in 0..count {
        if seen[slot] {
            continue;
        }
        seen[slot] = true;
        let mut group = vec![slot];
        let mut i = 0;
        while i < group.len() {
            for &next in &neighbors[group[i]] {
                if !seen[next] {
                    seen[next] = true;
                    group.push(next);
                }
            }
            i += 1;
        }
        groups.push(group);
    }
    groups
}

/// A bone of the temporary skeleton: where it came from (`entries`).
struct Entry {
    slot: usize,
    index: usize,
    scale: f64,
}

/// `remapRigReferences`: a rig relationship with its handle references moved
/// into the temporary skeleton's ids (a figure's root becomes the joint it
/// hangs from).
fn remap_rig(rig: &Rig, remap: &HashMap<u32, u32>) -> Rig {
    let map = |id: u32| remap.get(&id).copied().unwrap_or(id);
    fn weighted(w: &Weighted, map: &dyn Fn(u32) -> u32) -> Weighted {
        Weighted { origin: map(w.origin), anchors: w.anchors.iter().map(|&(joint, weight)| (map(joint), weight)).collect(), depth: w.depth.as_ref().map(|d| Box::new(weighted(d, map))) }
    }
    let mut out = rig.clone();
    out.spatial = rig.spatial.as_ref().map(|spatial| match spatial {
        Spatial::Skin { point, bindings, view_points } => {
            Spatial::Skin { point: *point, bindings: bindings.iter().map(|&(joint, weight)| (map(joint), weight)).collect(), view_points: view_points.clone() }
        }
        Spatial::Bone { parent, turn, pitch, origin, tip, motion } => {
            let mut motion = motion.clone();
            if let Some(driver) = motion.as_mut().and_then(|m| m.driver.as_mut()) {
                driver.0 = map(driver.0);
            }
            Spatial::Bone { parent: map(*parent), turn: map(*turn), pitch: map(*pitch), origin: *origin, tip: *tip, motion }
        }
    });
    out.weighted = rig.weighted.as_ref().map(|w| weighted(w, &map));
    if let Some(path) = out.path.as_mut() {
        path.driver = map(path.driver);
        path.origin = map(path.origin);
        path.pitch = path.pitch.map(map);
        path.source = path.source.map(map);
    }
    if let Some(connector) = out.connector.as_mut() {
        connector.start = map(connector.start);
        connector.end = map(connector.end);
    }
    if let Some(follow) = out.follow.as_mut() {
        follow.driver = map(follow.driver);
    }
    if let Some(lock) = out.lock.as_mut() {
        lock.reference = map(lock.reference);
        if let Some(secondary) = lock.secondary.as_mut() {
            secondary.0 = map(secondary.0);
        }
    }
    out
}

/// `joinedIKGraph(report, frame, slots)`: the temporary skeleton of one
/// assembly, its pose, and the bookkeeping to scatter a solution back.
struct Graph<'a> {
    segments: Vec<Segment>,
    polygons: Vec<Polyfill>,
    angles: Vec<f64>,
    lengths: Vec<f64>,
    bends: Vec<f64>,
    entries: Vec<Entry>,
    /// Per object slot: segment id in its figure → id in the skeleton.
    maps: HashMap<usize, HashMap<u32, u32>>,
    /// Per object slot: the skeleton joint it hangs from (0 = the assembly's root).
    roots: HashMap<usize, u32>,
    figures: HashMap<usize, &'a Figure>,
    root: [f64; 2],
}

impl<'a> Graph<'a> {
    fn new(frame: &'a Frame, slots: &[usize], figure_of: &dyn Fn(usize) -> Option<&'a Arc<Figure>>) -> Option<Graph<'a>> {
        // An assembly with a movieclip (or a missing figure) is not compiled.
        let mut figures: HashMap<usize, &'a Figure> = HashMap::new();
        for &slot in slots {
            let figure = figure_of(slot).filter(|f| f.movieclip.is_none())?;
            figures.insert(slot, figure);
        }
        let mut graph = Graph {
            segments: Vec::new(),
            polygons: Vec::new(),
            angles: Vec::new(),
            lengths: Vec::new(),
            bends: Vec::new(),
            entries: Vec::new(),
            maps: HashMap::new(),
            roots: HashMap::new(),
            figures,
            root: [0.0, 0.0],
        };
        for &slot in slots {
            let source = &frame.objects[slot];
            let figure = graph.figures[&slot];
            let mut ids = HashMap::new();
            for (index, s) in figure.segments.iter().enumerate() {
                let id = graph.entries.len() as u32 + 1;
                let scale = source.scale;
                ids.insert(s.id, id);
                graph.entries.push(Entry { slot, index, scale });
                // Parents and rig references are filled in once every figure has its ids.
                graph.segments.push(Segment { id, parent: 0, length: s.length * scale, bend: 0.0, rig: None, ..s.clone() });
                graph.angles.push(source.angles.get(index).copied().unwrap_or(s.angle));
                graph.lengths.push(source.lengths.as_ref().map(|l| l.get(index).copied().unwrap_or(s.length)).unwrap_or(s.length) * scale);
                graph.bends.push(match &source.bends {
                    Some(bends) => bends.get(index).copied().unwrap_or(0.0),
                    None => s.bend,
                });
            }
            graph.maps.insert(slot, ids);
        }
        for &slot in slots {
            graph.root_id(frame, slot, &mut Vec::new())?;
        }
        for &slot in slots {
            let figure = graph.figures[&slot];
            let ids = &graph.maps[&slot];
            let root_id = graph.roots[&slot];
            // `new Map([[0, rootId], ...ids])`: the figure's root is the joint it hangs from.
            let mut remap: HashMap<u32, u32> = ids.clone();
            remap.entry(0).or_insert(root_id);
            for s in &figure.segments {
                let i = ids[&s.id] as usize - 1;
                graph.segments[i].parent = if s.parent != 0 { ids.get(&s.parent).copied().unwrap_or(root_id) } else { root_id };
                graph.segments[i].rig = s.rig.as_ref().map(|rig| Arc::new(remap_rig(rig, &remap)));
            }
            for polygon in &figure.polyfills {
                // A handle the figure does not have maps to no bone (u32::MAX never owns a branch).
                let joints = polygon.joints.iter().map(|&id| if id != 0 { ids.get(&id).copied().unwrap_or(u32::MAX) } else { root_id }).collect();
                graph.polygons.push(Polyfill { joints, ..polygon.clone() });
            }
        }
        let root_slot = slots.iter().copied().find(|&slot| frame.objects[slot].join.is_none())?;
        graph.root = frame.objects[root_slot].root;
        Some(graph)
    }

    /// `rootId(slot)`: the skeleton joint an object hangs from. `None` where
    /// the JavaScript throws (a cyclic join, a join to a missing handle).
    fn root_id(&mut self, frame: &Frame, slot: usize, seen: &mut Vec<usize>) -> Option<u32> {
        if let Some(&id) = self.roots.get(&slot) {
            return Some(id);
        }
        if seen.contains(&slot) {
            return None;
        }
        seen.push(slot);
        let id = match frame.objects.get(slot)?.join {
            None => 0,
            Some(join) if join.handle != 0 => *self.maps.get(&(join.slot as usize))?.get(&join.handle)?,
            Some(join) => self.root_id(frame, join.slot as usize, seen)?,
        };
        self.roots.insert(slot, id);
        Some(id)
    }
}

/// `mapController(slot, handle, controller)`: a controller in the skeleton's
/// ids. Tables of the owner's own handles and of joined figures' handles merge
/// into one, and follower and attachment angles are completed for the
/// skeleton's branches.
fn map_controller(graph: &Graph, skeleton: &Skeleton, by_uid: &HashMap<u32, usize>, slot: usize, id: u32, c: &Controller) -> Controller {
    let own = &graph.maps[&slot];
    fn merge<T: Clone>(graph: &Graph, own: &HashMap<u32, u32>, by_uid: &HashMap<u32, usize>, local: Option<&BTreeMap<u32, T>>, joined: Option<&BTreeMap<u32, BTreeMap<u32, T>>>) -> BTreeMap<u32, T> {
        let mut offsets = BTreeMap::new();
        for (child, value) in local.into_iter().flatten() {
            if let Some(&mapped) = own.get(child) {
                offsets.insert(mapped, value.clone());
            }
        }
        for (uid, children) in joined.into_iter().flatten() {
            let Some(ids) = by_uid.get(uid).and_then(|slot| graph.maps.get(slot)) else { continue };
            for (child, value) in children {
                if let Some(&mapped) = ids.get(child) {
                    offsets.insert(mapped, value.clone());
                }
            }
        }
        offsets
    }
    let mut mapped = Controller {
        followers: merge(graph, own, by_uid, Some(&c.followers), c.joined_followers.as_ref()),
        attachment_offsets: Some(merge(graph, own, by_uid, c.attachment_offsets.as_ref(), c.joined_attachment_offsets.as_ref())),
        spatial_followers: Some(merge(graph, own, by_uid, c.spatial_followers.as_ref(), c.joined_spatial_followers.as_ref())),
        ..c.clone()
    };
    let followers = skeleton.followers(&graph.angles, id, &mapped);
    let attachments = skeleton.attachments(&graph.angles, id, &mapped, &followers);
    mapped.followers = followers.into_iter().collect();
    mapped.attachment_offsets = Some(attachments.into_iter().collect());
    mapped
}

/// Solves one assembly. `None` when it cannot be compiled or no pinned
/// controller ends at one of its handles.
fn solve_component<'a>(frame: &'a Frame, slots: &[usize], figure_of: &dyn Fn(usize) -> Option<&'a Arc<Figure>>) -> Option<Vec<(usize, Solution)>> {
    let graph = Graph::new(frame, slots, figure_of)?;
    let plan = plan_figure(&graph.segments, &graph.polygons);
    // With rig relationships anywhere in the assembly, every bone evaluates
    // through the figure it came from (`figure._rigSources`).
    let sources = plan.rig.is_some().then(|| RigSources {
        groups: slots
            .iter()
            .map(|&slot| {
                let figure = graph.figures[&slot];
                RigGroup {
                    figure,
                    object: &frame.objects[slot],
                    ids: graph.maps[&slot].clone(),
                    root_id: graph.roots[&slot],
                    plan: figure.plan.rig.clone().unwrap_or_else(|| Arc::new(plain_plan(figure.segments.len()))),
                }
            })
            .collect(),
        of: {
            let group_of: HashMap<usize, u32> = slots.iter().enumerate().map(|(g, &slot)| (slot, g as u32)).collect();
            graph.entries.iter().map(|e| (group_of[&e.slot], e.index as u32)).collect()
        },
    });
    let skeleton = Skeleton { segments: &graph.segments, plan: &plan, polygons: &graph.polygons, sources: sources.as_ref() };
    let by_uid: HashMap<u32, usize> = slots.iter().map(|&slot| (frame.objects[slot].uid, slot)).collect();
    let mut controllers = BTreeMap::new();
    let mut owners: HashMap<u32, (usize, u32)> = HashMap::new();
    for &slot in slots {
        for (&handle, c) in frame.objects[slot].ik.iter().flat_map(|table| table.controllers.iter()) {
            let Some(&id) = graph.maps[&slot].get(&handle) else { continue };
            controllers.insert(id, map_controller(&graph, &skeleton, &by_uid, slot, id, c));
            owners.insert(id, (slot, handle));
        }
    }
    let pose = Pose {
        scale: 1.0,
        angles: &graph.angles,
        lengths: Some(&graph.lengths),
        bends: Some(&graph.bends),
        mirror: false,
        figure_angle: graph.angles.first().copied().unwrap_or(0.0) - graph.segments.first().map(|s| s.angle).unwrap_or(0.0),
    };
    let solution = solve_object(&skeleton, &pose, graph.root, &Table { controllers })?;
    // `scatter(solution)`: back to each figure's own slots, ids and scale.
    let mut output: Vec<(usize, Solution)> = slots
        .iter()
        .map(|&slot| {
            let object: &Object = &frame.objects[slot];
            let figure = graph.figures[&slot];
            let angles = figure.segments.iter().enumerate().map(|(i, s)| object.angles.get(i).copied().unwrap_or(s.angle)).collect();
            (slot, Solution { angles, lengths: None, results: Vec::new() })
        })
        .collect();
    let at: HashMap<usize, usize> = slots.iter().enumerate().map(|(k, &slot)| (slot, k)).collect();
    for (i, entry) in graph.entries.iter().enumerate() {
        let result = &mut output[at[&entry.slot]].1;
        result.angles[entry.index] = solution.angles[i];
        if let Some(lengths) = &solution.lengths
            && entry.scale != 0.0
        {
            let own = result.lengths.get_or_insert_with(|| vec![f64::NAN; graph.figures[&entry.slot].segments.len()]);
            own[entry.index] = lengths[i] / entry.scale;
        }
    }
    for (id, outcome) in solution.results {
        let Some(&(slot, handle)) = owners.get(&id) else { continue };
        let chain = outcome.chain.iter().map(|&i| graph.entries[i].index).collect();
        output[at[&slot]].1.results.push((handle, Outcome { chain, ..outcome }));
    }
    Some(output)
}

/// `solveJoinedIK(report, frame)`: the derived pose of every object that
/// belongs to a joined assembly with a pinned controller, by object slot.
/// `figure_of` resolves a slot's figure (with its style overlay, if any).
pub fn solve<'a>(frame: &'a Frame, figure_of: &dyn Fn(usize) -> Option<&'a Arc<Figure>>) -> HashMap<usize, Solution> {
    let mut output = HashMap::new();
    if !frame.objects.iter().any(|o| o.join.is_some()) || !frame.objects.iter().any(|o| has_pinned(o)) {
        return output;
    }
    for slots in components(frame) {
        if slots.len() < 2 || !slots.iter().any(|&slot| has_pinned(&frame.objects[slot])) {
            continue;
        }
        if let Some(solved) = solve_component(frame, &slots, figure_of) {
            output.extend(solved);
        }
    }
    output
}
