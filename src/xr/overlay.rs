//! Hands, controllers and pointer beams. The runtime draws none of them for
//! an app (on the Quest nothing at all), so without this the user points
//! with an invisible hand. They are capsules (`overlay.wgsl`) drawn into a
//! projection layer of their own, submitted after the video and the panels,
//! so a hand reaching towards a panel stays in front of it. They show while
//! there is a panel to point at (the browser, or the control bar), not over a
//! video being watched.

use super::context::{XrContext, choose_color_format};
use super::input::{Ray, rotate};
use super::renderer::find_memory;
use anyhow::Context;
use ash::vk;
use openxr as xr;

const SHADER: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/overlay.spv"));
/// Two hands of 28 bones, two controllers and two beams fit with room to spare.
const MAX_CAPSULES: usize = 128;
/// Always available as a depth attachment (Vulkan requires it).
const DEPTH_FORMAT: vk::Format = vk::Format::D16_UNORM;

const HAND_COLOR: [f32; 3] = [0.78, 0.80, 0.86];
const HAND_ALPHA: f32 = 0.9;
const CONTROLLER_COLOR: [f32; 3] = [0.20, 0.20, 0.23];
const CONTROLLER_FACE_COLOR: [f32; 3] = [0.45, 0.46, 0.50];
const BEAM_COLOR: [f32; 3] = [1.0, 1.0, 1.0];
const BEAM_RADIUS: f32 = 0.0015;
/// A beam that hits no panel fades out over this length (metres).
pub const BEAM_REACH: f32 = 1.0;

/// A capsule: a segment with a radius, opaque or fading along its length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
    /// Linear RGB.
    pub color: [f32; 3],
    /// Opacity at `a` and at `b`.
    pub alpha: [f32; 2],
}

/// One capsule as the shader reads it (`Capsule` in overlay.wgsl).
#[repr(C)]
#[derive(Clone, Copy)]
struct Instance {
    a: [f32; 4],
    b: [f32; 4],
    color: [f32; 4],
}

impl From<&Capsule> for Instance {
    fn from(c: &Capsule) -> Self {
        Self {
            a: [c.a[0], c.a[1], c.a[2], c.radius],
            b: [c.b[0], c.b[1], c.b[2], c.alpha[1]],
            color: [c.color[0], c.color[1], c.color[2], c.alpha[0]],
        }
    }
}

/// Per-eye parameters; layout matches `Eye` in overlay.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct EyeParams {
    rows: [[f32; 4]; 3],
    tan: [f32; 4],
    origin: [f32; 4],
}

fn eye_params(view: &xr::View, width: u32) -> EyeParams {
    let q = view.pose.orientation;
    let axis = |v| {
        let r = rotate(q, v);
        [r[0], r[1], r[2], 0.0]
    };
    let fov = view.fov;
    let (left, right) = (fov.angle_left.tan(), fov.angle_right.tan());
    let p = view.pose.position;
    EyeParams {
        // The view's axes in the reference space are the rows of its inverse.
        rows: [
            axis([1.0, 0.0, 0.0]),
            axis([0.0, 1.0, 0.0]),
            axis([0.0, 0.0, 1.0]),
        ],
        tan: [left, right, fov.angle_down.tan(), fov.angle_up.tan()],
        origin: [p.x, p.y, p.z, (right - left) / width.max(1) as f32],
    }
}

fn to_world(pose: xr::Posef, local: [f32; 3]) -> [f32; 3] {
    let r = rotate(pose.orientation, local);
    let p = pose.position;
    [p.x + r[0], p.y + r[1], p.z + r[2]]
}

/// A stylised controller at its grip pose: a handle along the grip's Z axis
/// and a face across its front.
pub fn controller(grip: xr::Posef) -> [Capsule; 2] {
    let part = |a, b, radius, color| Capsule {
        a: to_world(grip, a),
        b: to_world(grip, b),
        radius,
        color,
        alpha: [1.0, 1.0],
    };
    [
        part(
            [0.0, -0.005, 0.045],
            [0.0, 0.005, -0.025],
            0.017,
            CONTROLLER_COLOR,
        ),
        part(
            [-0.022, 0.022, -0.045],
            [0.022, 0.022, -0.045],
            0.02,
            CONTROLLER_FACE_COLOR,
        ),
    ]
}

/// Bones between XR_EXT_hand_tracking joints: thumb, four fingers (with
/// their metacarpals, which make up the palm), and the knuckles' web.
const BONES: [(usize, usize); 27] = [
    // Thumb: wrist, metacarpal, proximal, distal, tip.
    (1, 2),
    (2, 3),
    (3, 4),
    (4, 5),
    // Index.
    (6, 7),
    (7, 8),
    (8, 9),
    (9, 10),
    // Middle.
    (11, 12),
    (12, 13),
    (13, 14),
    (14, 15),
    // Ring.
    (16, 17),
    (17, 18),
    (18, 19),
    (19, 20),
    // Little.
    (21, 22),
    (22, 23),
    (23, 24),
    (24, 25),
    // Wrist to the metacarpals.
    (1, 6),
    (1, 11),
    (1, 16),
    (1, 21),
    // Across the knuckles.
    (7, 12),
    (12, 17),
    (17, 22),
];

/// A hand from its tracked joints; bones with an untracked end are left out.
pub fn hand(joints: &xr::HandJointLocations) -> Vec<Capsule> {
    let tracked = |j: &xr::HandJointLocation| {
        j.location_flags
            .contains(xr::SpaceLocationFlags::POSITION_VALID)
    };
    let at = |j: &xr::HandJointLocation| [j.pose.position.x, j.pose.position.y, j.pose.position.z];
    BONES
        .iter()
        .filter_map(|&(from, to)| {
            let (a, b) = (&joints[from], &joints[to]);
            (tracked(a) && tracked(b)).then(|| Capsule {
                a: at(a),
                b: at(b),
                radius: (0.4 * (a.radius + b.radius)).clamp(0.004, 0.015),
                color: HAND_COLOR,
                alpha: [HAND_ALPHA, HAND_ALPHA],
            })
        })
        .collect()
}

/// The pointer: from just in front of the hand to the panel it hits
/// (`hit`, metres along the ray), or fading out over [`BEAM_REACH`].
pub fn beam(ray: &Ray, hit: Option<f32>, active: bool) -> Option<Capsule> {
    const START: f32 = 0.03;
    let length = hit.unwrap_or(BEAM_REACH);
    if length <= START {
        return None;
    }
    let point = |t: f32| {
        [
            ray.origin[0] + t * ray.direction[0],
            ray.origin[1] + t * ray.direction[1],
            ray.origin[2] + t * ray.direction[2],
        ]
    };
    let strength = if active { 0.7 } else { 0.3 };
    let end = if hit.is_some() { 0.5 * strength } else { 0.0 };
    Some(Capsule {
        a: point(START),
        b: point(length),
        radius: BEAM_RADIUS,
        color: BEAM_COLOR,
        alpha: [strength, end],
    })
}

struct Target {
    swapchain: xr::Swapchain<xr::Vulkan>,
    views: Vec<vk::ImageView>,
    framebuffers: Vec<vk::Framebuffer>,
    depth: vk::Image,
    depth_memory: vk::DeviceMemory,
    depth_view: vk::ImageView,
    width: u32,
    height: u32,
}

/// The overlay's own swapchains, pipeline and command buffer, so drawing it
/// doesn't depend on whether the video was drawn this frame.
pub struct Overlay {
    device: ash::Device,
    queue: vk::Queue,
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    instances: vk::Buffer,
    instance_memory: vk::DeviceMemory,
    mapped: *mut Instance,
    targets: Vec<Target>,
}

impl Overlay {
    pub fn new(ctx: &XrContext) -> anyhow::Result<Self> {
        let device = ctx.device.clone();
        let memory_types = unsafe {
            ctx.vk
                .get_physical_device_memory_properties(ctx.physical_device)
        };
        let color_format = choose_color_format(&ctx.swapchain_formats()?)?;
        unsafe {
            let attachments = [
                vk::AttachmentDescription::default()
                    .format(color_format)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL),
                vk::AttachmentDescription::default()
                    .format(DEPTH_FORMAT)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                    .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                    .initial_layout(vk::ImageLayout::UNDEFINED)
                    .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL),
            ];
            let color_ref = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
            let depth_ref = vk::AttachmentReference::default()
                .attachment(1)
                .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);
            let subpass = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_ref)
                .depth_stencil_attachment(&depth_ref)];
            let render_pass = device.create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpass),
                None,
            )?;

            let push = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .size(size_of::<EyeParams>() as u32)];
            let pipeline_layout = device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().push_constant_ranges(&push),
                None,
            )?;

            let code = ash::util::read_spv(&mut std::io::Cursor::new(SHADER))?;
            let module = device
                .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
            let stages = [
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::VERTEX)
                    .module(module)
                    .name(c"vs_main"),
                vk::PipelineShaderStageCreateInfo::default()
                    .stage(vk::ShaderStageFlags::FRAGMENT)
                    .module(module)
                    .name(c"fs_main"),
            ];
            let bindings = [vk::VertexInputBindingDescription::default()
                .binding(0)
                .stride(size_of::<Instance>() as u32)
                .input_rate(vk::VertexInputRate::INSTANCE)];
            let attributes = [0u32, 1, 2].map(|location| {
                vk::VertexInputAttributeDescription::default()
                    .location(location)
                    .binding(0)
                    .format(vk::Format::R32G32B32A32_SFLOAT)
                    .offset(location * 16)
            });
            let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attributes);
            let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
                .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);
            let viewport = vk::PipelineViewportStateCreateInfo::default()
                .viewport_count(1)
                .scissor_count(1);
            let raster = vk::PipelineRasterizationStateCreateInfo::default()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .line_width(1.0);
            let multisample = vk::PipelineMultisampleStateCreateInfo::default()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            // Edge pixels write the far plane, so they must pass against it.
            let depth = vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(true)
                .depth_compare_op(vk::CompareOp::LESS_OR_EQUAL);
            // Premultiplied alpha.
            let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD)
                .color_write_mask(vk::ColorComponentFlags::RGBA)];
            let blend =
                vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
            let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
            let dynamic =
                vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
            let pipeline_info = [vk::GraphicsPipelineCreateInfo::default()
                .stages(&stages)
                .vertex_input_state(&vertex_input)
                .input_assembly_state(&assembly)
                .viewport_state(&viewport)
                .rasterization_state(&raster)
                .multisample_state(&multisample)
                .depth_stencil_state(&depth)
                .color_blend_state(&blend)
                .dynamic_state(&dynamic)
                .layout(pipeline_layout)
                .render_pass(render_pass)];
            let pipeline = device
                .create_graphics_pipelines(vk::PipelineCache::null(), &pipeline_info, None)
                .map_err(|(_, e)| e)?[0];
            device.destroy_shader_module(module, None);

            let command_pool = device.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(ctx.queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?;
            let command = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(command_pool)
                    .command_buffer_count(1),
            )?[0];
            let fence = device.create_fence(&vk::FenceCreateInfo::default(), None)?;

            let size = (MAX_CAPSULES * size_of::<Instance>()) as u64;
            let instances = device.create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(size)
                    .usage(vk::BufferUsageFlags::VERTEX_BUFFER),
                None,
            )?;
            let requirements = device.get_buffer_memory_requirements(instances);
            let instance_memory = device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(find_memory(
                        &memory_types,
                        requirements.memory_type_bits,
                        vk::MemoryPropertyFlags::HOST_VISIBLE
                            | vk::MemoryPropertyFlags::HOST_COHERENT,
                    )?),
                None,
            )?;
            device.bind_buffer_memory(instances, instance_memory, 0)?;
            let mapped = device.map_memory(instance_memory, 0, size, vk::MemoryMapFlags::empty())?
                as *mut Instance;

            let mut overlay = Self {
                device,
                queue: ctx.queue,
                render_pass,
                pipeline_layout,
                pipeline,
                command_pool,
                command,
                fence,
                instances,
                instance_memory,
                mapped,
                targets: Vec::new(),
            };
            for (i, view) in ctx.views.iter().enumerate() {
                let target = overlay
                    .create_target(ctx, view, color_format, &memory_types)
                    .with_context(|| format!("Create overlay swapchain {i}"))?;
                overlay.targets.push(target);
            }
            Ok(overlay)
        }
    }

    fn create_target(
        &self,
        ctx: &XrContext,
        view: &xr::ViewConfigurationView,
        color_format: vk::Format,
        memory_types: &vk::PhysicalDeviceMemoryProperties,
    ) -> anyhow::Result<Target> {
        let (width, height) = (
            view.recommended_image_rect_width,
            view.recommended_image_rect_height,
        );
        let swapchain = ctx.session.create_swapchain(&xr::SwapchainCreateInfo {
            create_flags: xr::SwapchainCreateFlags::EMPTY,
            usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT,
            format: color_format.as_raw() as u32,
            sample_count: 1,
            width,
            height,
            face_count: 1,
            array_size: 1,
            mip_count: 1,
        })?;
        unsafe {
            let depth = self.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(DEPTH_FORMAT)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(
                        vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                            | vk::ImageUsageFlags::TRANSIENT_ATTACHMENT,
                    ),
                None,
            )?;
            let requirements = self.device.get_image_memory_requirements(depth);
            // Tiled GPUs (the Quest's) keep a transient depth buffer on chip.
            let memory_type = find_memory(
                memory_types,
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::LAZILY_ALLOCATED,
            )
            .or_else(|_| {
                find_memory(
                    memory_types,
                    requirements.memory_type_bits,
                    vk::MemoryPropertyFlags::DEVICE_LOCAL,
                )
            })?;
            let depth_memory = self.device.allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type),
                None,
            )?;
            self.device.bind_image_memory(depth, depth_memory, 0)?;
            let depth_view = self.device.create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(depth)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(DEPTH_FORMAT)
                    .subresource_range(
                        vk::ImageSubresourceRange::default()
                            .aspect_mask(vk::ImageAspectFlags::DEPTH)
                            .level_count(1)
                            .layer_count(1),
                    ),
                None,
            )?;
            let mut views = Vec::new();
            let mut framebuffers = Vec::new();
            for image in swapchain.enumerate_images()? {
                let view = self.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(vk::Handle::from_raw(image))
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(color_format)
                        .subresource_range(
                            vk::ImageSubresourceRange::default()
                                .aspect_mask(vk::ImageAspectFlags::COLOR)
                                .level_count(1)
                                .layer_count(1),
                        ),
                    None,
                )?;
                let attachments = [view, depth_view];
                let framebuffer = self.device.create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(self.render_pass)
                        .attachments(&attachments)
                        .width(width)
                        .height(height)
                        .layers(1),
                    None,
                )?;
                views.push(view);
                framebuffers.push(framebuffer);
            }
            Ok(Target {
                swapchain,
                views,
                framebuffers,
                depth,
                depth_memory,
                depth_view,
                width,
                height,
            })
        }
    }

    /// Draws `capsules` for both eyes (`views` from xrLocateViews) and waits
    /// for the GPU; [`Overlay::projection_views`] then submits the result.
    pub fn draw(&mut self, views: &[xr::View], capsules: &[Capsule]) -> anyhow::Result<()> {
        let count = capsules.len().min(MAX_CAPSULES);
        for (i, capsule) in capsules[..count].iter().enumerate() {
            // SAFETY: the mapped buffer holds MAX_CAPSULES instances, and the
            // GPU finished reading it at the end of the previous draw.
            unsafe { self.mapped.add(i).write(Instance::from(capsule)) };
        }
        let mut indices = Vec::with_capacity(self.targets.len());
        for target in &mut self.targets {
            let index = target.swapchain.acquire_image()?;
            target.swapchain.wait_image(xr::Duration::INFINITE)?;
            indices.push(index);
        }
        unsafe {
            let device = &self.device;
            device.reset_command_buffer(self.command, vk::CommandBufferResetFlags::empty())?;
            device.begin_command_buffer(
                self.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            for ((target, view), &index) in self.targets.iter().zip(views).zip(&indices) {
                let extent = vk::Extent2D {
                    width: target.width,
                    height: target.height,
                };
                let clear = [
                    vk::ClearValue {
                        color: vk::ClearColorValue { float32: [0.0; 4] },
                    },
                    vk::ClearValue {
                        depth_stencil: vk::ClearDepthStencilValue {
                            depth: 1.0,
                            stencil: 0,
                        },
                    },
                ];
                device.cmd_begin_render_pass(
                    self.command,
                    &vk::RenderPassBeginInfo::default()
                        .render_pass(self.render_pass)
                        .framebuffer(target.framebuffers[index as usize])
                        .render_area(vk::Rect2D {
                            offset: vk::Offset2D::default(),
                            extent,
                        })
                        .clear_values(&clear),
                    vk::SubpassContents::INLINE,
                );
                device.cmd_set_viewport(
                    self.command,
                    0,
                    &[vk::Viewport {
                        x: 0.0,
                        y: 0.0,
                        width: extent.width as f32,
                        height: extent.height as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    }],
                );
                device.cmd_set_scissor(
                    self.command,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D::default(),
                        extent,
                    }],
                );
                if count > 0 {
                    device.cmd_bind_pipeline(
                        self.command,
                        vk::PipelineBindPoint::GRAPHICS,
                        self.pipeline,
                    );
                    device.cmd_bind_vertex_buffers(self.command, 0, &[self.instances], &[0]);
                    let params = eye_params(view, target.width);
                    let bytes = std::slice::from_raw_parts(
                        &params as *const EyeParams as *const u8,
                        size_of::<EyeParams>(),
                    );
                    device.cmd_push_constants(
                        self.command,
                        self.pipeline_layout,
                        vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                        0,
                        bytes,
                    );
                    device.cmd_draw(self.command, 14, count as u32, 0, 0);
                }
                device.cmd_end_render_pass(self.command);
            }
            device.end_command_buffer(self.command)?;
            let commands = [self.command];
            device.queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&commands)],
                self.fence,
            )?;
            device.wait_for_fences(&[self.fence], true, u64::MAX)?;
            device.reset_fences(&[self.fence])?;
        }
        for target in &mut self.targets {
            target.swapchain.release_image()?;
        }
        Ok(())
    }

    /// The overlay's views for a projection layer, at the poses and fields
    /// of view it was drawn with.
    pub fn projection_views(
        &self,
        views: &[(xr::Posef, xr::Fovf)],
    ) -> Vec<xr::CompositionLayerProjectionView<'_, xr::Vulkan>> {
        views
            .iter()
            .zip(&self.targets)
            .map(|(&(pose, fov), target)| {
                xr::CompositionLayerProjectionView::new()
                    .pose(pose)
                    .fov(fov)
                    .sub_image(
                        xr::SwapchainSubImage::new()
                            .swapchain(&target.swapchain)
                            .image_array_index(0)
                            .image_rect(xr::Rect2Di {
                                offset: xr::Offset2Di { x: 0, y: 0 },
                                extent: xr::Extent2Di {
                                    width: target.width as i32,
                                    height: target.height as i32,
                                },
                            }),
                    )
            })
            .collect()
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            let device = &self.device;
            let _ = device.device_wait_idle();
            for target in &self.targets {
                for &framebuffer in &target.framebuffers {
                    device.destroy_framebuffer(framebuffer, None);
                }
                for &view in &target.views {
                    device.destroy_image_view(view, None);
                }
                device.destroy_image_view(target.depth_view, None);
                device.destroy_image(target.depth, None);
                device.free_memory(target.depth_memory, None);
            }
            device.unmap_memory(self.instance_memory);
            device.destroy_buffer(self.instances, None);
            device.free_memory(self.instance_memory, None);
            device.destroy_fence(self.fence, None);
            device.destroy_command_pool(self.command_pool, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_render_pass(self.render_pass, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beam_reaches_the_panel_or_fades_out() {
        let ray = Ray {
            origin: [0.0, 1.0, 0.0],
            direction: [0.0, 0.0, -1.0],
        };
        let hit = beam(&ray, Some(2.0), true).unwrap();
        assert_eq!(hit.b, [0.0, 1.0, -2.0]);
        assert!(hit.alpha[1] > 0.0, "visible up to the panel");
        let free = beam(&ray, None, false).unwrap();
        assert_eq!(free.b, [0.0, 1.0, -BEAM_REACH]);
        assert_eq!(free.alpha[1], 0.0);
        assert!(
            free.alpha[0] < hit.alpha[0],
            "the other hand's beam is fainter"
        );
        // A panel right at the hand: no beam inside the controller.
        assert!(beam(&ray, Some(0.01), true).is_none());
    }

    #[test]
    fn controller_follows_its_grip() {
        let grip = xr::Posef {
            orientation: xr::Quaternionf::IDENTITY,
            position: xr::Vector3f {
                x: 0.2,
                y: 1.0,
                z: -0.3,
            },
        };
        let [handle, face] = controller(grip);
        assert!((handle.a[0] - 0.2).abs() < 1e-6 && (handle.a[2] + 0.255).abs() < 1e-6);
        assert!(
            face.a[2] < handle.a[2],
            "the face is in front of the handle"
        );
    }

    #[test]
    fn untracked_joints_draw_no_bones() {
        let mut joints = [xr::HandJointLocation {
            location_flags: xr::SpaceLocationFlags::POSITION_VALID,
            pose: xr::Posef::IDENTITY,
            radius: 0.01,
        }; xr::HAND_JOINT_COUNT];
        assert_eq!(hand(&joints).len(), BONES.len());
        // Losing the wrist drops the thumb's base and the palm's four bones.
        joints[1].location_flags = xr::SpaceLocationFlags::EMPTY;
        assert_eq!(hand(&joints).len(), BONES.len() - 5);
    }

    #[test]
    fn eye_axes_are_the_inverse_rotation() {
        // Turned 90° left: the world's -X is straight ahead (view -Z).
        let half = std::f32::consts::FRAC_PI_4;
        let view = xr::View {
            pose: xr::Posef {
                orientation: xr::Quaternionf {
                    x: 0.0,
                    y: half.sin(),
                    z: 0.0,
                    w: half.cos(),
                },
                position: xr::Vector3f::default(),
            },
            fov: xr::Fovf {
                angle_left: -0.8,
                angle_right: 0.8,
                angle_up: 0.8,
                angle_down: -0.8,
            },
        };
        let e = eye_params(&view, 1000);
        let d = [-1.0, 0.0, 0.0];
        let v: Vec<f32> = e
            .rows
            .iter()
            .map(|r| r[0] * d[0] + r[1] * d[1] + r[2] * d[2])
            .collect();
        assert!((v[2] + 1.0).abs() < 1e-5, "{v:?}");
        assert!(e.origin[3] > 0.0);
    }
}
