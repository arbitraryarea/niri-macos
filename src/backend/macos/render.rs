//! # Metal/OpenGL Rendering 🎨
//!
//! This module handles GPU rendering using Metal (preferred) or OpenGL.
//! We composite Wayland client surfaces to a Metal texture and present
//! it to the CAMetalLayer in our Cocoa window.
//!
//! ## Rendering Pipeline
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                    Render Frame                         │
//! │  ┌───────────────────────────────────────────────────┐  │
//! │  │  1. Acquire drawable from CAMetalLayer           │  │
//! │  └─────────────────────┬─────────────────────────────┘  │
//! │                        │                                │
//! │  ┌─────────────────────▼─────────────────────────────┐  │
//! │  │  2. Create command buffer                        │  │
//! │  └─────────────────────┬─────────────────────────────┘  │
//! │                        │                                │
//! │  ┌─────────────────────▼─────────────────────────────┐  │
//! │  │  3. Render background                            │  │
//! │  └─────────────────────┬─────────────────────────────┘  │
//! │                        │                                │
//! │  ┌─────────────────────▼─────────────────────────────┐  │
//! │  │  4. Composite Wayland surfaces                   │  │
//! │  └─────────────────────┬─────────────────────────────┘  │
//! │                        │                                │
//! │  ┌─────────────────────▼─────────────────────────────┐  │
//! │  │  5. Render UI overlays                           │  │
//! │  └─────────────────────┬─────────────────────────────┘  │
//! │                        │                                │
//! │  ┌─────────────────────▼─────────────────────────────┐  │
//! │  │  6. Present drawable                             │  │
//! │  └───────────────────────────────────────────────────┘  │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! Running Crayons Ltd. - "60 FPS or your money back*"
//! (*Money back guarantee not available)

use std::sync::Arc;

#[cfg(target_os = "macos")]
use metal::{
    Device, CommandQueue, MTLPixelFormat, MTLLoadAction, MTLStoreAction,
    RenderPassDescriptor, RenderPipelineState, Library, MTLResourceOptions,
    Buffer, Texture, TextureDescriptor, MTLTextureUsage,
};
#[cfg(target_os = "macos")]
use cocoa::base::id;

/// 🎨 Metal Renderer
///
/// Handles all Metal rendering operations.
pub struct MetalRenderer {
    #[cfg(target_os = "macos")]
    device: Device,
    #[cfg(target_os = "macos")]
    command_queue: CommandQueue,
    #[cfg(target_os = "macos")]
    pipeline_state: Option<RenderPipelineState>,
    #[cfg(target_os = "macos")]
    library: Option<Library>,

    /// Clear color (RGBA)
    clear_color: [f32; 4],
    /// Whether VSync is enabled
    vsync_enabled: bool,
    /// Target frame rate (for ProMotion displays)
    target_fps: u32,
    /// Debug tint enabled
    debug_tint: bool,
    /// Frame counter for statistics
    frame_count: u64,
}

/// 🖼️ Render Target
#[derive(Debug)]
pub struct RenderTarget {
    pub width: u32,
    pub height: u32,
    pub scale_factor: f64,
    #[cfg(target_os = "macos")]
    pub texture: Option<Texture>,
}

/// 📊 Render Statistics
#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    pub frame_time_ms: f64,
    pub draw_calls: u32,
    pub triangles: u32,
    pub textures_used: u32,
}

impl MetalRenderer {
    /// Creates a new Metal renderer.
    #[cfg(target_os = "macos")]
    pub fn new() -> Result<Self, String> {
        info!("🎨 Initializing Metal renderer...");

        // Get the default Metal device
        let device = Device::system_default()
            .ok_or_else(|| "No Metal-capable device found".to_string())?;

        info!("🎨 Using Metal device: {}", device.name());

        // Create command queue
        let command_queue = device.new_command_queue();

        // Create shader library
        let library = create_shader_library(&device)?;

        // Create pipeline state
        let pipeline_state = create_pipeline_state(&device, &library)?;

        info!("🎨 Metal renderer initialized successfully!");

        Ok(Self {
            device,
            command_queue,
            pipeline_state: Some(pipeline_state),
            library: Some(library),
            clear_color: [0.1, 0.1, 0.1, 1.0], // Dark gray
            vsync_enabled: true,
            target_fps: 60,
            debug_tint: false,
            frame_count: 0,
        })
    }

    /// Non-macOS stub
    #[cfg(not(target_os = "macos"))]
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            clear_color: [0.1, 0.1, 0.1, 1.0],
            vsync_enabled: true,
            target_fps: 60,
            debug_tint: false,
            frame_count: 0,
        })
    }

    /// Configures the renderer for a Metal layer.
    #[cfg(target_os = "macos")]
    pub fn configure_layer(&self, layer: id, width: u32, height: u32, scale: f64) {
        use objc::{msg_send, sel, sel_impl};

        unsafe {
            // Set the device
            let _: () = msg_send![layer, setDevice: self.device.as_ptr()];

            // Set pixel format
            let _: () = msg_send![layer, setPixelFormat: MTLPixelFormat::BGRA8Unorm];

            // Set drawable size
            let size = core_graphics::geometry::CGSize::new(
                width as f64 * scale,
                height as f64 * scale,
            );
            let _: () = msg_send![layer, setDrawableSize: size];

            // Enable display sync (VSync)
            let _: () = msg_send![layer, setDisplaySyncEnabled: self.vsync_enabled];

            // Set contents scale
            let _: () = msg_send![layer, setContentsScale: scale];

            info!(
                "🎨 Configured Metal layer: {}x{} @ {}x scale",
                width, height, scale
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn configure_layer(&self, _layer: (), _width: u32, _height: u32, _scale: f64) {}

    /// Begins a new frame.
    #[cfg(target_os = "macos")]
    pub fn begin_frame(&mut self, layer: id) -> Option<FrameContext> {
        use objc::{msg_send, sel, sel_impl};

        unsafe {
            // Get the next drawable
            let drawable: id = msg_send![layer, nextDrawable];
            if drawable.is_null() {
                warn!("Failed to get next drawable");
                return None;
            }

            // Create command buffer
            let command_buffer = self.command_queue.new_command_buffer();

            // Get the drawable's texture
            let texture: id = msg_send![drawable, texture];

            Some(FrameContext {
                drawable,
                texture,
                command_buffer: command_buffer.to_owned(),
            })
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn begin_frame(&mut self, _layer: ()) -> Option<FrameContext> {
        Some(FrameContext {})
    }

    /// Renders the frame content.
    #[cfg(target_os = "macos")]
    pub fn render_frame(&mut self, ctx: &FrameContext, elements: &[RenderElement]) {
        // Create render pass descriptor
        let render_pass = RenderPassDescriptor::new();

        // Configure color attachment
        let color_attachment = render_pass.color_attachments().object_at(0).unwrap();
        color_attachment.set_texture(Some(&Texture::from_ptr(ctx.texture as *mut _)));
        color_attachment.set_load_action(MTLLoadAction::Clear);
        color_attachment.set_store_action(MTLStoreAction::Store);
        color_attachment.set_clear_color(metal::MTLClearColor::new(
            self.clear_color[0] as f64,
            self.clear_color[1] as f64,
            self.clear_color[2] as f64,
            self.clear_color[3] as f64,
        ));

        // Create render encoder
        let encoder = ctx.command_buffer.new_render_command_encoder(&render_pass);

        if let Some(ref pipeline) = self.pipeline_state {
            encoder.set_render_pipeline_state(pipeline);
        }

        // Render each element
        for element in elements {
            self.render_element(&encoder, element);
        }

        encoder.end_encoding();

        self.frame_count += 1;
    }

    #[cfg(not(target_os = "macos"))]
    pub fn render_frame(&mut self, _ctx: &FrameContext, _elements: &[RenderElement]) {
        self.frame_count += 1;
    }

    /// Renders a single element.
    #[cfg(target_os = "macos")]
    fn render_element(&self, _encoder: &metal::RenderCommandEncoderRef, element: &RenderElement) {
        // In a full implementation, this would:
        // 1. Set vertex buffers for the element's geometry
        // 2. Set texture for the element
        // 3. Set transform matrix
        // 4. Draw the element

        trace!(
            "Rendering element at ({}, {}) size {}x{}",
            element.x, element.y, element.width, element.height
        );
    }

    /// Ends the frame and presents it.
    #[cfg(target_os = "macos")]
    pub fn end_frame(&mut self, ctx: FrameContext) {
        use objc::{msg_send, sel, sel_impl};

        unsafe {
            // Schedule presentation
            let _: () = msg_send![ctx.command_buffer.as_ptr(), presentDrawable: ctx.drawable];

            // Commit the command buffer
            ctx.command_buffer.commit();
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn end_frame(&mut self, _ctx: FrameContext) {}

    /// Sets the clear color.
    pub fn set_clear_color(&mut self, color: [f32; 4]) {
        self.clear_color = color;
    }

    /// Enables or disables VSync.
    pub fn set_vsync(&mut self, enabled: bool) {
        self.vsync_enabled = enabled;
    }

    /// Sets the target frame rate (for ProMotion).
    pub fn set_target_fps(&mut self, fps: u32) {
        self.target_fps = fps;
    }

    /// Enables or disables debug tint.
    pub fn set_debug_tint(&mut self, enabled: bool) {
        self.debug_tint = enabled;
    }

    /// Gets render statistics.
    pub fn stats(&self) -> RenderStats {
        RenderStats {
            frame_time_ms: 0.0, // Would be calculated from frame timing
            draw_calls: 0,
            triangles: 0,
            textures_used: 0,
        }
    }

    /// Gets the frame count.
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

/// 🖼️ Frame Context
///
/// Holds resources for rendering a single frame.
pub struct FrameContext {
    #[cfg(target_os = "macos")]
    drawable: id,
    #[cfg(target_os = "macos")]
    texture: id,
    #[cfg(target_os = "macos")]
    command_buffer: metal::CommandBuffer,
}

/// 📦 Render Element
///
/// Represents something to be rendered (window, UI element, etc.)
#[derive(Debug, Clone)]
pub struct RenderElement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub opacity: f32,
    pub texture_id: Option<u64>,
}

// ============================================================================
// 🛠️ Shader Creation
// ============================================================================

#[cfg(target_os = "macos")]
fn create_shader_library(device: &Device) -> Result<Library, String> {
    // Metal Shading Language source
    let shader_source = r#"
        #include <metal_stdlib>
        using namespace metal;

        struct VertexIn {
            float2 position [[attribute(0)]];
            float2 texCoord [[attribute(1)]];
        };

        struct VertexOut {
            float4 position [[position]];
            float2 texCoord;
        };

        struct Uniforms {
            float4x4 transform;
            float opacity;
        };

        vertex VertexOut vertex_main(
            VertexIn in [[stage_in]],
            constant Uniforms &uniforms [[buffer(1)]]
        ) {
            VertexOut out;
            out.position = uniforms.transform * float4(in.position, 0.0, 1.0);
            out.texCoord = in.texCoord;
            return out;
        }

        fragment float4 fragment_main(
            VertexOut in [[stage_in]],
            texture2d<float> texture [[texture(0)]],
            sampler textureSampler [[sampler(0)]],
            constant Uniforms &uniforms [[buffer(1)]]
        ) {
            float4 color = texture.sample(textureSampler, in.texCoord);
            color.a *= uniforms.opacity;
            return color;
        }

        // Solid color shader for backgrounds
        fragment float4 fragment_solid(
            VertexOut in [[stage_in]],
            constant float4 &color [[buffer(0)]]
        ) {
            return color;
        }
    "#;

    let library = device
        .new_library_with_source(shader_source, &metal::CompileOptions::new())
        .map_err(|e| format!("Failed to compile shaders: {}", e))?;

    Ok(library)
}

#[cfg(target_os = "macos")]
fn create_pipeline_state(
    device: &Device,
    library: &Library,
) -> Result<RenderPipelineState, String> {
    let vertex_func = library
        .get_function("vertex_main", None)
        .map_err(|e| format!("Failed to get vertex function: {}", e))?;

    let fragment_func = library
        .get_function("fragment_main", None)
        .map_err(|e| format!("Failed to get fragment function: {}", e))?;

    let pipeline_desc = metal::RenderPipelineDescriptor::new();
    pipeline_desc.set_vertex_function(Some(&vertex_func));
    pipeline_desc.set_fragment_function(Some(&fragment_func));

    // Configure color attachment
    let attachment = pipeline_desc
        .color_attachments()
        .object_at(0)
        .unwrap();
    attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);

    // Enable blending for alpha compositing
    attachment.set_blending_enabled(true);
    attachment.set_source_rgb_blend_factor(metal::MTLBlendFactor::SourceAlpha);
    attachment.set_destination_rgb_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);
    attachment.set_source_alpha_blend_factor(metal::MTLBlendFactor::One);
    attachment.set_destination_alpha_blend_factor(metal::MTLBlendFactor::OneMinusSourceAlpha);

    let pipeline_state = device
        .new_render_pipeline_state(&pipeline_desc)
        .map_err(|e| format!("Failed to create pipeline state: {}", e))?;

    Ok(pipeline_state)
}

// ============================================================================
// 🧪 Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_element() {
        let element = RenderElement {
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 100.0,
            opacity: 1.0,
            texture_id: None,
        };

        assert_eq!(element.width, 100.0);
        assert_eq!(element.opacity, 1.0);
    }

    #[test]
    fn test_render_stats() {
        let stats = RenderStats::default();
        assert_eq!(stats.draw_calls, 0);
        assert_eq!(stats.triangles, 0);
    }
}
