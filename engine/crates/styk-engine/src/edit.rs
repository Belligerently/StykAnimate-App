//! The frame the editor is showing while it edits.
//!
//! Editing commands still run in JavaScript, which evaluates the pose it is
//! changing. The interface passes that pose here (only the objects that changed
//! since the last frame) and the engine builds its geometry and draw list. The
//! document's frames are not involved, so a gesture never waits for a commit.
//!
//! Onion skins arrive the same way: each ghost is another pose, kept by an id the
//! interface chooses, with its geometry built once and reused while the ghost
//! does not change.

use std::collections::HashMap;
use std::sync::Arc;

use styk_doc::Value;
use styk_eval::drawlist::{DrawList, Ghost, Overrides, encode_composed};
use styk_eval::eval::Frame;
use styk_eval::geometry::{Scene, build};
use styk_eval::load::Loader;
use styk_eval::onion::{GhostPlace, compose};

use crate::Generation;

/// Ghost poses kept at most. The interface names the ones it shows each frame;
/// the least recently shown are dropped beyond this.
const MAX_GHOSTS: usize = 48;

struct Pose {
    frame: Frame,
    /// Geometry of `frame`, built when first drawn.
    scene: Option<Arc<Scene>>,
    /// The draw counter when this pose was last shown.
    used: u64,
    /// What the scene camera does to `frame`, for the generation it was worked out for.
    shot: Option<(u64, Arc<styk_eval::parallax::Parallax>)>,
}

impl Pose {
    fn new(frame: Frame, used: u64) -> Pose {
        Pose { frame, scene: None, used, shot: None }
    }

    /// How far parallax layers move this pose's objects while the scene camera shows it
    /// (`parallaxResolver`), worked out once per document generation.
    fn parallax(&mut self, generation: &Generation) -> Option<Arc<styk_eval::parallax::Parallax>> {
        if let Some((id, parallax)) = &self.shot
            && *id == generation.id
        {
            return Some(parallax.clone());
        }
        let parallax = styk_eval::parallax::resolve(&generation.prepared.doc, &self.frame, self.frame.camera.as_ref())?;
        self.shot = Some((generation.id, parallax.clone()));
        Some(parallax)
    }
}

/// An edit frame, its onion skins, and what was last built from them. Owned by the thread that draws.
#[derive(Default)]
pub struct EditScene {
    loader: Option<Loader>,
    current: Option<Pose>,
    ghosts: HashMap<u32, Pose>,
    /// The document lineage and generation the kept poses and geometry belong to.
    lineage: u64,
    generation: u64,
    draws: u64,
}

/// One onion skin of a frame.
#[derive(Clone, Debug)]
pub struct Onion<'a> {
    /// The id its pose was given in [`EditScene::update_ghost`].
    pub id: u32,
    pub opacity: f64,
    pub tint: Option<[u8; 3]>,
    pub show_on_top: bool,
    pub before_uid: Option<u32>,
    /// Slots of the ghost that are not drawn.
    pub hidden: &'a [u32],
}

/// The editor's session state for one frame.
#[derive(Clone, Debug, Default)]
pub struct EditOptions<'a> {
    /// Slots not drawn.
    pub hidden: &'a [u32],
    /// `(slot, [dx, dy])` of dragged objects.
    pub translations: &'a [(u32, [f64; 2])],
    /// ARGB canvas colour replacing the scene background.
    pub background: Option<u32>,
    /// Onion skins, far to near.
    pub onions: &'a [Onion<'a>],
    /// Draw without effects (the editor's effects preview is off).
    pub bypass_effects: bool,
    /// The frame is shown through the scene camera (playback preview), which moves parallax layers.
    pub camera: bool,
}

fn flags(slots: &[u32], count: usize) -> Vec<bool> {
    let mut hidden = vec![false; count];
    for &slot in slots {
        if let Some(flag) = hidden.get_mut(slot as usize) {
            *flag = true;
        }
    }
    hidden
}

impl EditScene {
    /// Starts over when the document is another project; drops built geometry when its definitions changed.
    fn follow(&mut self, generation: &Generation) {
        if self.lineage != generation.lineage || self.loader.is_none() {
            self.loader = Some(Loader::new());
            self.current = None;
            self.ghosts.clear();
            self.lineage = generation.lineage;
        }
        if self.generation != generation.id {
            // Figures may have changed: geometry is rebuilt from the kept poses.
            self.generation = generation.id;
            if let Some(pose) = &mut self.current {
                pose.scene = None;
            }
            for pose in self.ghosts.values_mut() {
                pose.scene = None;
            }
        }
    }

    /// Reads a pose. Its effects are the ones the editor composed into it
    /// (clips included), never sampled again here: see `docs/engine/vfx.md`.
    fn parse(&mut self, generation: &Generation, text: &[u8], previous: Option<&Frame>) -> Result<Frame, String> {
        let value: Value = styk_doc::json::parse(text).map_err(|e| e.to_string())?;
        let loader = self.loader.as_mut().expect("follow() ran");
        let doc = &generation.prepared.doc;
        let mut frame = loader.edit_frame(&value, previous, doc).map_err(|e| e.to_string())?;
        let fields = frame.pose_vfx.is_some() || frame.objects.iter().any(|o| o.vfx.is_some() || o.vfx_connected.is_some());
        if fields || generation.prepared.has_vfx {
            frame.vfx = styk_eval::vfx::frame::composed(doc, &frame);
        }
        Ok(frame)
    }

    /// Replaces the frame. `text` is a keyframe-shaped JSON record whose `objects`
    /// entries are poses or `null` for "unchanged since the last call".
    pub fn update(&mut self, generation: &Generation, text: &[u8]) -> Result<(), String> {
        self.follow(generation);
        let previous = self.current.take();
        let frame = self.parse(generation, text, previous.as_ref().map(|pose| &pose.frame))?;
        self.current = Some(Pose::new(frame, self.draws));
        Ok(())
    }

    /// Replaces the pose of ghost `id`, with the same `null` convention against its previous pose.
    pub fn update_ghost(&mut self, generation: &Generation, id: u32, text: &[u8]) -> Result<(), String> {
        self.follow(generation);
        let previous = self.ghosts.remove(&id);
        let frame = self.parse(generation, text, previous.as_ref().map(|pose| &pose.frame))?;
        self.ghosts.insert(id, Pose::new(frame, self.draws));
        Ok(())
    }

    pub fn has_frame(&self) -> bool {
        self.current.is_some()
    }

    /// The camera of the edit frame.
    pub fn camera(&self) -> Option<styk_eval::doc::Camera> {
        self.current.as_ref().and_then(|pose| pose.frame.camera)
    }

    /// The draw list of the current frame under `options`. `Err` names a ghost the
    /// interface asked for and never sent.
    pub fn draw_list(&mut self, generation: &Generation, options: &EditOptions<'_>) -> Result<Option<Arc<DrawList>>, String> {
        self.follow(generation);
        self.draws += 1;
        let doc = &generation.prepared.doc;
        let scene_of = |pose: &mut Pose| pose.scene.get_or_insert_with(|| Arc::new(build(doc, &pose.frame))).clone();
        let Some(current) = self.current.as_mut() else { return Ok(None) };
        let scene = scene_of(current);
        let hidden = flags(options.hidden, current.frame.objects.len());
        // Through the scene camera, parallax layers move after any drag (onion skins are
        // never shown with the camera on, so they take no parallax).
        let parallax = if options.camera { current.parallax(generation) } else { None };
        let moved = parallax.map(|p| p.translations(options.translations));
        let translations = moved.as_deref().unwrap_or(options.translations);
        let overrides = Overrides { hidden: &hidden, translations, background: options.background, bypass_effects: options.bypass_effects };
        if options.onions.is_empty() {
            let frame_scene = scene;
            return Ok(Some(Arc::new(encode_composed(doc, &generation.assets, &frame_scene, &overrides, &[], None))));
        }
        let mut scenes = Vec::with_capacity(options.onions.len());
        for onion in options.onions {
            let pose = self.ghosts.get_mut(&onion.id).ok_or_else(|| format!("onion skin {} was never sent", onion.id))?;
            pose.used = self.draws;
            let ghost_scene = scene_of(pose);
            scenes.push((ghost_scene, flags(onion.hidden, pose.frame.objects.len())));
        }
        let current = self.current.as_ref().expect("checked above");
        let places: Vec<GhostPlace<'_>> = options
            .onions
            .iter()
            .zip(&scenes)
            .map(|(onion, (ghost_scene, ghost_hidden))| GhostPlace {
                frame: &self.ghosts[&onion.id].frame,
                scene: ghost_scene,
                hidden: ghost_hidden,
                show_on_top: onion.show_on_top,
                before_uid: onion.before_uid,
            })
            .collect();
        let order = compose(doc.layers.as_deref(), &current.frame, &scene, &places);
        let ghosts: Vec<Ghost<'_>> = options
            .onions
            .iter()
            .zip(&scenes)
            .map(|(onion, (ghost_scene, ghost_hidden))| Ghost { scene: ghost_scene, opacity: onion.opacity, tint: onion.tint, hidden: ghost_hidden })
            .collect();
        let list = Arc::new(encode_composed(doc, &generation.assets, &scene, &overrides, &ghosts, Some(&order)));
        drop(places);
        if self.ghosts.len() > MAX_GHOSTS {
            let mut by_age: Vec<(u64, u32)> = self.ghosts.iter().map(|(&id, pose)| (pose.used, id)).collect();
            by_age.sort_unstable();
            for &(_, id) in &by_age[..self.ghosts.len() - MAX_GHOSTS] {
                self.ghosts.remove(&id);
            }
        }
        Ok(Some(list))
    }

    /// Whether ghost `id` is still kept (the interface resends a dropped one whole).
    pub fn has_ghost(&self, id: u32) -> bool {
        self.ghosts.contains_key(&id)
    }
}
