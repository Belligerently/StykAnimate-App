//! Depth runs on the CPU: the scene and expectations of the GPU test
//! (styk-render/tests/depth.rs), so both renderers are held to one picture.

#[path = "../../styk-render/tests/support/depth_scene.rs"]
mod depth_scene;

#[test]
fn depth_runs_hide_faces_behind_and_keep_painter_order() {
    for anti_alias in [true, false] {
        let view = styk_raster::View { matrix: [1.0, 0.0, 0.0, 1.0, 0.0, 0.0], width: 64, height: 64, artboard: [0.0, 0.0, 64.0, 64.0], clip_to_artboard: false, clear: [0.0; 4], anti_alias };
        let pixmap = styk_raster::render(&depth_scene::scene(), &styk_raster::Images::default(), &view).expect("a frame");
        for (x, y, rgb, why) in depth_scene::EXPECTED {
            let c = pixmap.pixel(x, y).expect("inside").demultiply();
            assert_eq!([c.red(), c.green(), c.blue()], rgb, "{why} (antialiased: {anti_alias})");
        }
    }
}
