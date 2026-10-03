//! One frame: isolation layer passes by round and depth, then the frame pass.

use styk_draw::K_COMPOSITE;
use styk_eval::drawlist::layer_effects;

use crate::effects::executor::{Executor, Frame, Run};
use crate::effects::{self, plan, Slot};
use crate::isolation::{page_size, plan_rounds};
use crate::resources::{buffer, GpuGroup, GpuView};
use crate::{Renderer, Stats, ViewParams, FORMAT, SAMPLES, UNIFORM_STRIDE};

/// One effect run of a frame: a filtered group's layer at `at` on `page` of its depth's atlas.
struct Work {
    round: usize,
    depth: u32,
    page: u32,
    at: [u32; 2],
    size: [u32; 2],
    run: Run,
}

/// What passes of a list with depth runs draw with: the flat and faces pipelines
/// (`DepthPipelines`) and the ranges of the list's prims that are faces.
struct DepthRuns {
    flat: wgpu::RenderPipeline,
    faces: wgpu::RenderPipeline,
    flat_add: wgpu::RenderPipeline,
    ranges: Vec<(u32, u32)>,
}

/// Where the list's prims add light (styk-draw `F_ADD`) and the additive pipeline.
struct Additive {
    pipeline: wgpu::RenderPipeline,
    ranges: Vec<(u32, u32)>,
}

impl DepthRuns {
    /// Whether prims `range` hold any face: only such passes get a depth buffer.
    fn within(&self, range: std::ops::Range<u32>) -> bool {
        let first = self.ranges.partition_point(|&(_, end)| end <= range.start);
        self.ranges.get(first).is_some_and(|&(start, _)| start < range.end)
    }
}

/// A pass's depth buffer: each run resets the depth it uses, so it is never kept.
fn depth_attachment(view: &wgpu::TextureView) -> wgpu::RenderPassDepthStencilAttachment<'_> {
    wgpu::RenderPassDepthStencilAttachment { view, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }
}

/// Draws prims `range` of the list in one instanced draw, split where generated effects
/// add light (`additive`) and where depth-tested faces need the depth pipeline (`depth`:
/// the flat and faces pipelines of a pass with a depth buffer, and the list's face ranges).
fn draw_prims(pass: &mut wgpu::RenderPass<'_>, base: &wgpu::RenderPipeline, additive: Option<&Additive>, depth: Option<&DepthRuns>, range: std::ops::Range<u32>) {
    let Some(add) = additive else { return draw_painted(pass, base, depth, range) };
    let mut at = range.start;
    let first = add.ranges.partition_point(|&(_, end)| end <= range.start);
    for &(start, end) in &add.ranges[first..] {
        if start >= range.end {
            break;
        }
        if start > at {
            draw_painted(pass, base, depth, at..start);
        }
        pass.set_pipeline(depth.map_or(&add.pipeline, |d| &d.flat_add));
        pass.draw(0..6, start.max(at)..end.min(range.end));
        at = end.min(range.end);
    }
    if range.end > at {
        draw_painted(pass, base, depth, at..range.end);
    }
}

/// [`draw_prims`] for prims blended over the target.
fn draw_painted(pass: &mut wgpu::RenderPass<'_>, base: &wgpu::RenderPipeline, depth: Option<&DepthRuns>, range: std::ops::Range<u32>) {
    let Some(DepthRuns { flat, faces, ranges, .. }) = depth else {
        pass.set_pipeline(base);
        pass.draw(0..6, range);
        return;
    };
    let mut at = range.start;
    let first = ranges.partition_point(|&(_, end)| end <= range.start);
    for &(start, end) in &ranges[first..] {
        if start >= range.end {
            break;
        }
        if start > at {
            pass.set_pipeline(flat);
            pass.draw(0..6, at..start);
        }
        pass.set_pipeline(faces);
        pass.draw(0..6, start.max(at)..end.min(range.end));
        at = end.min(range.end);
    }
    if range.end > at {
        pass.set_pipeline(flat);
        pass.draw(0..6, at..range.end);
    }
}

impl Renderer {
    /// A multisampled depth buffer of `size` for passes with depth runs.
    fn depth_buffer(device: &wgpu::Device, slot: &mut Option<(wgpu::Texture, [u32; 2])>, size: [u32; 2]) -> wgpu::TextureView {
        if slot.as_ref().is_none_or(|s| s.1 != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("styk depth"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: SAMPLES,
                dimension: wgpu::TextureDimension::D2,
                format: crate::pipeline::DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *slot = Some((texture, size));
        }
        slot.as_ref().unwrap().0.create_view(&Default::default())
    }

    pub(crate) fn ensure_output(&mut self, size: [u32; 2]) {
        if self.output.as_ref().is_some_and(|o| o.3 == size) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("styk frame"),
            size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("styk blit"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.nearest) },
            ],
        });
        self.output = Some((texture, view, bind, size));
    }

    pub(crate) fn msaa(device: &wgpu::Device, slot: &mut Option<(wgpu::Texture, [u32; 2])>, size: [u32; 2]) -> wgpu::TextureView {
        if slot.as_ref().is_none_or(|s| s.1 != size) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("styk msaa"),
                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: SAMPLES,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *slot = Some((texture, size));
        }
        slot.as_ref().unwrap().0.create_view(&Default::default())
    }

    /// Renders the current draw list into the offscreen frame.
    pub fn render(&mut self, encoder: &mut wgpu::CommandEncoder, view: &ViewParams, size: [u32; 2]) {
        let size = [size[0].max(1), size[1].max(1)];
        self.ensure_output(size);
        let Some(list) = self.list.clone() else {
            self.clear_only(encoder, view);
            return;
        };
        if self.bind0.is_none() {
            self.rebind();
        }
        let n = list.groups.len();
        let max_depth = list.groups.iter().map(|g| g.depth).max().unwrap_or(0);
        // Where every group is drawn for this view (effects::plan), then where it lands in the atlas.
        let planned = plan::plan(&list, &plan::View {
            matrix: view.matrix,
            size,
            zoom: view.zoom,
            pan: view.pan,
            camera_view: view.camera_view,
            max_texture: self.device.limits().max_texture_dimension_2d,
        });
        let sizes: Vec<[u32; 2]> = planned.iter().map(plan::Placement::size).collect();
        let largest = sizes.iter().skip(1).fold([0u32, 0u32], |a, s| [a[0].max(s[0]), a[1].max(s[1])]);
        // Pages hold the frame and grow for larger layers (effect layers at the editor's density, the
        // camera image); after 240 frames that all fit the frame they shrink back to it.
        let wanted = page_size(size, largest);
        self.page_small = if wanted == size { self.page_small + 1 } else { 0 };
        if self.page_frame != size || self.page_small > 240 {
            self.page_frame = size;
            self.page = size;
            self.page_small = 0;
        }
        self.page = [self.page[0].max(wanted[0]), self.page[1].max(wanted[1])];
        let page = self.page;
        let rounds = plan_rounds(&list, &sizes, page);
        let mut slots: Vec<Option<Slot>> = vec![None; n];
        for round in &rounds {
            for &(g, slot) in &round.placed {
                slots[g] = Some(slot);
            }
        }
        let mut placed = vec![GpuGroup::default(); n];
        let mut matrices: Vec<plan::Affine> = vec![view.matrix; n];
        for g in 1..n {
            let (record, matrix) = effects::record(&planned[g], matrices[list.groups[g].parent as usize], slots[g]);
            placed[g] = GpuGroup(record);
            matrices[g] = matrix;
        }
        let downscaled = rounds.iter().any(|round| round.downscaled);
        if self.groups.write(&self.device, &self.queue, bytemuck::cast_slice(&placed)) {
            self.rebind();
        }
        // Effect runs: recorded now (their uniforms are written once), encoded after their depth's layer pass.
        let mut work: Vec<Work> = Vec::new();
        if list.layers.iter().any(|layer| layer.effects.is_some()) {
            let executor = self.executor.get_or_insert_with(|| Executor::new(&self.device));
            executor.begin();
            let mut biggest = [0u32, 0u32];
            for (r, round) in rounds.iter().enumerate() {
                for &(g, slot) in &round.placed {
                    let Some(filter) = planned[g].filter else { continue };
                    let stack = list.layers.get(list.groups[g].layer as usize).map_or(&[][..], layer_effects);
                    if stack.is_empty() {
                        continue;
                    }
                    let s = sizes[g];
                    let at = [(s[0] as f64 * slot.scale).ceil().max(1.0) as u32, (s[1] as f64 * slot.scale).ceil().max(1.0) as u32];
                    let f = effects::scaled(filter, slot.scale);
                    let run = executor.record(stack, Frame { scale: f.scale, origin: f.origin, output_size: f.output_size }, at);
                    biggest = [biggest[0].max(at[0]), biggest[1].max(at[1])];
                    work.push(Work { round: r, depth: list.groups[g].depth, page: slot.page, at: [slot.offset[0] as u32, slot.offset[1] as u32], size: at, run });
                }
            }
            executor.prepare(&self.device, &self.queue, biggest);
        } else if let Some(executor) = &mut self.executor {
            executor.begin();
            executor.prepare(&self.device, &self.queue, [0, 0]);
        }
        // Each group's prims are one run inside its depth's pass (the encoder emits them by group).
        let mut runs = vec![(u32::MAX, 0u32); n];
        for (i, p) in list.prims.iter().enumerate() {
            let run = &mut runs[p.group as usize];
            run.0 = run.0.min(i as u32);
            run.1 = i as u32 + 1;
        }
        // One uniform slot per layer pass (round, depth, page) and one for the frame.
        let mut passes: Vec<(usize, u32, u32, u32, u32)> = Vec::new(); // round, depth, page, start, end
        for (r, round) in rounds.iter().enumerate() {
            for depth in (1..=max_depth).rev() {
                let (mut start, mut end) = (u32::MAX, 0u32);
                for g in round.first..round.end {
                    if list.groups[g].depth == depth && runs[g].0 < runs[g].1 {
                        start = start.min(runs[g].0);
                        end = end.max(runs[g].1);
                    }
                }
                if start >= end {
                    continue;
                }
                for page in 0..round.pages_per_depth[depth as usize] {
                    passes.push((r, depth, page, start, end));
                }
            }
        }
        let root = list.passes.iter().find(|p| p.depth == 0).map(|p| (p.start, p.end)).unwrap_or((0, 0));
        let slots_needed = passes.len() as u64 + 1;
        if slots_needed > self.uniform_slots {
            self.uniform_slots = slots_needed.next_power_of_two();
            self.uniforms = buffer(&self.device, "styk views", UNIFORM_STRIDE * self.uniform_slots, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
            self.rebind();
        }
        let mut uniform_data = vec![0u8; (UNIFORM_STRIDE * slots_needed) as usize];
        for (i, &(_, depth, page_index, _, _)) in passes.iter().enumerate() {
            let v = view.gpu([page[0] as f32, page[1] as f32, page_index as f32, depth as f32]);
            let o = i * UNIFORM_STRIDE as usize;
            uniform_data[o..o + std::mem::size_of::<GpuView>()].copy_from_slice(bytemuck::bytes_of(&v));
        }
        let v = view.gpu([size[0] as f32, size[1] as f32, 0.0, 0.0]);
        let o = passes.len() * UNIFORM_STRIDE as usize;
        uniform_data[o..o + std::mem::size_of::<GpuView>()].copy_from_slice(bytemuck::bytes_of(&v));
        self.queue.write_buffer(&self.uniforms, 0, &uniform_data);

        for depth in 1..=max_depth {
            let pages = rounds.iter().map(|round| round.pages_per_depth[depth as usize]).max().unwrap_or(0);
            if pages > 0 {
                self.ensure_layers(depth as usize % 2, page, pages);
            }
        }
        // Where the frame stops for the next round: before the first composite of a group the next round draws.
        let mut round_of = vec![usize::MAX; n];
        for (r, round) in rounds.iter().enumerate() {
            round_of[round.first..round.end].fill(r);
        }
        let bg = list.background_prims.min(root.1 - root.0);
        let mut stops = vec![root.1; rounds.len()];
        let mut next = 1;
        for i in root.0 + bg..root.1 {
            let p = &list.prims[i as usize];
            if p.kind & 255 != K_COMPOSITE {
                continue;
            }
            let r = round_of.get(p.b[0].to_bits() as usize).copied().unwrap_or(usize::MAX);
            while next < rounds.len() && r != usize::MAX && r >= next {
                stops[next - 1] = i;
                next += 1;
            }
        }
        let bind0 = self.bind0.clone().unwrap();
        // Depth runs: depth-buffered passes, created only for lists that have faces.
        let depth_runs = (!self.depth_faces.is_empty()).then(|| {
            let ranges = self.depth_faces.clone();
            let pipelines = self.depth_pipelines();
            DepthRuns { flat: pipelines.flat.clone(), faces: pipelines.faces.clone(), flat_add: pipelines.flat_add.clone(), ranges }
        });
        let additive = (!self.add_ranges.is_empty()).then(|| Additive { pipeline: self.additive_pipeline(), ranges: self.add_ranges.clone() });
        let (mut layer_depth, mut frame_depth) = (false, false);
        let mut from = root.0 + bg;
        for (r, _) in rounds.iter().enumerate() {
            for depth in (1..=max_depth).rev() {
                for (i, &(round, pass_depth, page_index, start, end)) in passes.iter().enumerate() {
                    if round != r || pass_depth != depth {
                        continue;
                    }
                    let parity = depth as usize % 2;
                    let target = self.layers[parity].texture.create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: page_index,
                        array_layer_count: Some(1),
                        ..Default::default()
                    });
                    let msaa = Self::msaa(&self.device, &mut self.msaa_layer, page);
                    let face_runs = depth_runs.as_ref().filter(|r| r.within(start..end));
                    let depth_view = face_runs.map(|_| Self::depth_buffer(&self.device, &mut self.depth_layer, page));
                    layer_depth |= depth_view.is_some();
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("styk layer pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &msaa,
                            depth_slice: None,
                            resolve_target: Some(&target),
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
                        })],
                        depth_stencil_attachment: depth_view.as_ref().map(depth_attachment),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_bind_group(0, &bind0, &[(i as u64 * UNIFORM_STRIDE) as u32]);
                    pass.set_bind_group(1, &self.sprite_bind, &[]);
                    pass.set_bind_group(2, &self.layer_binds[(depth as usize + 1) % 2], &[]);
                    pass.set_vertex_buffer(0, self.prims.slice(..));
                    draw_prims(&mut pass, &self.pipeline, additive.as_ref(), face_runs, start..end);
                }
                // The filtered groups of this depth, before the next depth composites them.
                if let Some(executor) = &mut self.executor {
                    let atlas = &self.layers[depth as usize % 2].texture;
                    for w in work.iter().filter(|w| w.round == r && w.depth == depth) {
                        let extent = wgpu::Extent3d { width: w.size[0], height: w.size[1], depth_or_array_layers: 1 };
                        let place = wgpu::TexelCopyTextureInfo { texture: atlas, mip_level: 0, origin: wgpu::Origin3d { x: w.at[0], y: w.at[1], z: w.page }, aspect: wgpu::TextureAspect::All };
                        encoder.copy_texture_to_texture(place, executor.slot_texture(&self.device, 0).as_image_copy(), extent);
                        executor.encode(&self.device, encoder, &w.run);
                        encoder.copy_texture_to_texture(executor.slot_texture(&self.device, w.run.result).as_image_copy(), place, extent);
                    }
                }
            }
            // The frame, up to the next round's first composite. The multisampled frame is kept between rounds.
            let last = r + 1 == rounds.len();
            let until = if last { root.1 } else { stops[r] };
            let msaa = Self::msaa(&self.device, &mut self.msaa_main, size);
            // Faces sit in raster entries, so a frame pass seldom holds any.
            let face_runs = depth_runs.as_ref().filter(|r| r.within(root.0..root.1));
            let depth_view = face_runs.map(|_| Self::depth_buffer(&self.device, &mut self.depth_main, size));
            frame_depth |= depth_view.is_some();
            let output = &self.output.as_ref().unwrap().1;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("styk frame pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa,
                    depth_slice: None,
                    resolve_target: if last { Some(output) } else { None },
                    ops: wgpu::Operations {
                        load: if r == 0 {
                            wgpu::LoadOp::Clear(wgpu::Color { r: view.clear[0], g: view.clear[1], b: view.clear[2], a: view.clear[3] })
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: if last { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
                depth_stencil_attachment: depth_view.as_ref().map(depth_attachment),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &bind0, &[(passes.len() as u64 * UNIFORM_STRIDE) as u32]);
            pass.set_bind_group(1, &self.sprite_bind, &[]);
            pass.set_bind_group(2, &self.layer_binds[1], &[]);
            pass.set_vertex_buffer(0, self.prims.slice(..));
            if r == 0 && bg > 0 {
                draw_prims(&mut pass, &self.pipeline, additive.as_ref(), face_runs, root.0..root.0 + bg);
            }
            if until > from {
                if view.clip_to_artboard {
                    let a = view.artboard;
                    let x0 = a[0].clamp(0.0, size[0] as f64).floor() as u32;
                    let y0 = a[1].clamp(0.0, size[1] as f64).floor() as u32;
                    let x1 = a[2].clamp(0.0, size[0] as f64).ceil() as u32;
                    let y1 = a[3].clamp(0.0, size[1] as f64).ceil() as u32;
                    if x1 > x0 && y1 > y0 {
                        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
                        draw_prims(&mut pass, &self.pipeline, additive.as_ref(), face_runs, from..until);
                    }
                } else {
                    draw_prims(&mut pass, &self.pipeline, additive.as_ref(), face_runs, from..until);
                }
            }
            from = until;
        }
        // Depth buffers live while frames use them (a document with 3D faces).
        if !layer_depth {
            self.depth_layer = None;
        }
        if !frame_depth {
            self.depth_main = None;
        }
        let fx = self.executor.as_ref().map(|e| e.stats).unwrap_or_default();
        // An atlas always has two layers at least (resources::array_texture).
        let layer_bytes = self.layers.iter().map(|a| a.size[0] as u64 * a.size[1] as u64 * 4 * a.pages.max(2) as u64).sum();
        // Four samples of four bytes (depth24plus) per pixel, frame and layer pages.
        let depth_bytes = [&self.depth_main, &self.depth_layer].iter().filter_map(|d| d.as_ref()).map(|d| d.1[0] as u64 * d.1[1] as u64 * 4 * SAMPLES as u64).sum();
        self.stats = Stats {
            prims: list.prims.len() as u32,
            groups: n as u32 - 1,
            passes: passes.len() as u32 + rounds.len() as u32,
            downscaled,
            rounds: rounds.len() as u32,
            effect_layers: fx.layers,
            effect_passes: fx.passes,
            effect_bytes: fx.slot_bytes,
            layer_bytes,
            depth_bytes,
            material_bytes: self.materials.bytes(),
        };
    }

    fn clear_only(&mut self, encoder: &mut wgpu::CommandEncoder, view: &ViewParams) {
        let output = &self.output.as_ref().unwrap().1;
        let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("styk clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: view.clear[0], g: view.clear[1], b: view.clear[2], a: view.clear[3] }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

}
