//! The frame the editor is showing while it edits.
//!
//! Editing commands still run in JavaScript, which evaluates the pose it is
//! changing. The interface passes that pose here (only the objects that changed
//! since the last frame) and the engine builds its geometry and draw list. The
//! document is not involved, so a gesture never waits for a commit.

use std::sync::Arc;

use styk_doc::Value;
use styk_eval::drawlist::{DrawList, Overrides, encode_with};
use styk_eval::eval::Frame;
use styk_eval::geometry::build;
use styk_eval::load::Loader;

use crate::Generation;

/// An edit frame and the draw list last built from it. Owned by the thread that draws.
#[derive(Default)]
pub struct EditScene {
    loader: Option<Loader>,
    frame: Option<Frame>,
    /// The document lineage the kept objects belong to.
    lineage: u64,
    list: Option<Arc<DrawList>>,
}

/// The editor's session state for one frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct EditOptions<'a> {
    /// Slots not drawn.
    pub hidden: &'a [u32],
    /// `(slot, dx, dy)` of dragged objects.
    pub translations: &'a [(u32, [f64; 2])],
    /// ARGB canvas colour replacing the scene background.
    pub background: Option<u32>,
}

impl EditScene {
    /// Replaces the frame. `text` is a keyframe-shaped JSON record whose `objects`
    /// entries are poses or `null` for "unchanged since the last call".
    pub fn update(&mut self, generation: &Generation, text: &[u8]) -> Result<(), String> {
        if self.lineage != generation.lineage || self.loader.is_none() {
            self.loader = Some(Loader::new());
            self.frame = None;
            self.lineage = generation.lineage;
        }
        let value: Value = styk_doc::json::parse(text).map_err(|e| e.to_string())?;
        let loader = self.loader.as_mut().expect("set above");
        let frame = loader.edit_frame(&value, self.frame.as_ref(), &generation.prepared.doc).map_err(|e| e.to_string())?;
        self.frame = Some(frame);
        self.list = None;
        Ok(())
    }

    pub fn has_frame(&self) -> bool {
        self.frame.is_some()
    }

    /// The camera of the edit frame.
    pub fn camera(&self) -> Option<styk_eval::doc::Camera> {
        self.frame.as_ref().and_then(|frame| frame.camera)
    }

    /// The draw list of the current frame under `options`.
    pub fn draw_list(&mut self, generation: &Generation, options: &EditOptions<'_>) -> Option<Arc<DrawList>> {
        let frame = self.frame.as_ref()?;
        let doc = &generation.prepared.doc;
        let scene = build(doc, frame);
        let mut hidden = vec![false; frame.objects.len()];
        for &slot in options.hidden {
            if let Some(flag) = hidden.get_mut(slot as usize) {
                *flag = true;
            }
        }
        let overrides = Overrides { hidden: &hidden, translations: options.translations, background: options.background };
        let list = Arc::new(encode_with(doc, &generation.assets, &scene, &overrides));
        self.list = Some(list.clone());
        Some(list)
    }
}
