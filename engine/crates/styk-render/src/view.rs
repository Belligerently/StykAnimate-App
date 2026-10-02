//! How a frame maps onto its output.

use crate::resources::GpuView;

/// How the scene maps onto the output, in output pixels.
#[derive(Clone, Copy, Debug)]
pub struct ViewParams {
    /// World → output pixels: x' = a*x + c*y + e, y' = b*x + d*y + f.
    pub matrix: [f64; 6],
    pub artboard: [f64; 4],
    pub clip_to_artboard: bool,
    pub clear: [f64; 4],
    /// Workspace zoom (output pixels per world unit) and pan, before any scene
    /// camera: the camera image of scene effects is sized and placed from them.
    pub zoom: f64,
    pub pan: [f64; 2],
    /// `matrix` shows the scene through its camera (playback), not the neutral workspace.
    pub camera_view: bool,
}

impl ViewParams {
    pub(crate) fn gpu(&self, target: [f32; 4]) -> GpuView {
        let [a, b, c, d, e, f] = self.matrix;
        let det = a * d - b * c;
        let (ia, ib, ic, id) = (d / det, -b / det, -c / det, a / det);
        let ie = -(ia * e + ic * f);
        let i_f = -(ib * e + id * f);
        GpuView {
            m: [a as f32, b as f32, c as f32, d as f32],
            t: [e as f32, f as f32, det.abs().sqrt() as f32, 0.0],
            inv: [ia as f32, ib as f32, ic as f32, id as f32],
            inv_t: [ie as f32, i_f as f32, 0.0, 0.0],
            target,
            artboard: self.artboard.map(|v| v as f32),
        }
    }
}

