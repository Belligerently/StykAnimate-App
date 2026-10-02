//! One frame: isolation layer passes by round and depth, then the frame pass.

use styk_draw::K_COMPOSITE;

use crate::isolation::plan_rounds;
use crate::resources::{buffer, GpuGroup, GpuView};
use crate::{Renderer, Stats, ViewParams, FORMAT, SAMPLES, UNIFORM_STRIDE};

impl Renderer {
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
        let rounds = plan_rounds(&list, view, size);
        let mut placed = vec![GpuGroup::default(); n];
        for round in &rounds {
            for &(g, entry) in &round.placed {
                placed[g] = entry;
            }
        }
        let downscaled = rounds.iter().any(|round| round.downscaled);
        if self.groups.write(&self.device, &self.queue, bytemuck::cast_slice(&placed)) {
            self.rebind();
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
        let slots = passes.len() as u64 + 1;
        if slots > self.uniform_slots {
            self.uniform_slots = slots.next_power_of_two();
            self.uniforms = buffer(&self.device, "styk views", UNIFORM_STRIDE * self.uniform_slots, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
            self.rebind();
        }
        let mut uniform_data = vec![0u8; (UNIFORM_STRIDE * slots) as usize];
        for (i, &(_, depth, page, _, _)) in passes.iter().enumerate() {
            let v = view.gpu([size[0] as f32, size[1] as f32, page as f32, depth as f32]);
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
                self.ensure_layers(depth as usize % 2, size, pages);
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
        let mut from = root.0 + bg;
        for (r, _) in rounds.iter().enumerate() {
            for (i, &(round, depth, page, start, end)) in passes.iter().enumerate() {
                if round != r {
                    continue;
                }
                let parity = depth as usize % 2;
                let target = self.layers[parity].texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: page,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let msaa = Self::msaa(&self.device, &mut self.msaa_layer, size);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("styk layer pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &msaa,
                        depth_slice: None,
                        resolve_target: Some(&target),
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.pipeline);
                pass.set_bind_group(0, &bind0, &[(i as u64 * UNIFORM_STRIDE) as u32]);
                pass.set_bind_group(1, &self.sprite_bind, &[]);
                pass.set_bind_group(2, &self.layer_binds[(depth as usize + 1) % 2], &[]);
                pass.set_vertex_buffer(0, self.prims.slice(..));
                pass.draw(0..6, start..end);
            }
            // The frame, up to the next round's first composite. The multisampled frame is kept between rounds.
            let last = r + 1 == rounds.len();
            let until = if last { root.1 } else { stops[r] };
            let msaa = Self::msaa(&self.device, &mut self.msaa_main, size);
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind0, &[(passes.len() as u64 * UNIFORM_STRIDE) as u32]);
            pass.set_bind_group(1, &self.sprite_bind, &[]);
            pass.set_bind_group(2, &self.layer_binds[1], &[]);
            pass.set_vertex_buffer(0, self.prims.slice(..));
            if r == 0 && bg > 0 {
                pass.draw(0..6, root.0..root.0 + bg);
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
                        pass.draw(0..6, from..until);
                    }
                } else {
                    pass.draw(0..6, from..until);
                }
            }
            from = until;
        }
        self.stats = Stats { prims: list.prims.len() as u32, groups: n as u32 - 1, passes: passes.len() as u32 + rounds.len() as u32, downscaled, rounds: rounds.len() as u32 };
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
