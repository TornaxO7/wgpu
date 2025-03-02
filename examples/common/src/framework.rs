#[cfg(target_arch = "wasm32")]
use std::str::FromStr;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use web_sys::{ImageBitmapRenderingContext, OffscreenCanvas};
use wgpu::{WasmNotSend, WasmNotSync};
use wgpu_test::GpuTestConfiguration;
use winit::{
    event::{self, KeyEvent, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    keyboard::{Key, NamedKey},
    window::Window,
};

#[allow(dead_code)]
pub fn cast_slice<T>(data: &[T]) -> &[u8] {
    use std::{mem::size_of_val, slice::from_raw_parts};

    unsafe { from_raw_parts(data.as_ptr() as *const u8, size_of_val(data)) }
}

#[allow(dead_code)]
pub enum ShaderStage {
    Vertex,
    Fragment,
    Compute,
}

pub trait Example: 'static + Sized {
    fn optional_features() -> wgpu::Features {
        wgpu::Features::empty()
    }

    fn required_features() -> wgpu::Features {
        wgpu::Features::empty()
    }

    fn required_downlevel_capabilities() -> wgpu::DownlevelCapabilities {
        wgpu::DownlevelCapabilities {
            flags: wgpu::DownlevelFlags::empty(),
            shader_model: wgpu::ShaderModel::Sm5,
            ..wgpu::DownlevelCapabilities::default()
        }
    }

    fn required_limits() -> wgpu::Limits {
        wgpu::Limits::downlevel_webgl2_defaults() // These downlevel limits will allow the code to run on all possible hardware
    }

    fn init(
        config: &wgpu::SurfaceConfiguration,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Self;

    fn resize(
        &mut self,
        config: &wgpu::SurfaceConfiguration,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    );

    fn update(&mut self, event: WindowEvent);

    fn render(&mut self, view: &wgpu::TextureView, device: &wgpu::Device, queue: &wgpu::Queue);
}

struct Setup {
    _window: Window,
    event_loop: EventLoop<()>,
    instance: wgpu::Instance,
    size: winit::dpi::PhysicalSize<u32>,
    surface: wgpu::Surface,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    #[cfg(target_arch = "wasm32")]
    offscreen_canvas_setup: Option<OffscreenCanvasSetup>,
}

#[cfg(target_arch = "wasm32")]
struct OffscreenCanvasSetup {
    offscreen_canvas: OffscreenCanvas,
    bitmap_renderer: ImageBitmapRenderingContext,
}

async fn setup<E: Example>(title: &str) -> Setup {
    #[cfg(not(target_arch = "wasm32"))]
    {
        env_logger::init();
    };

    let event_loop = EventLoop::new().unwrap();
    let mut builder = winit::window::WindowBuilder::new();
    builder = builder.with_title(title);
    #[cfg(windows_OFF)] // TODO
    {
        use winit::platform::windows::WindowBuilderExtWindows;
        builder = builder.with_no_redirection_bitmap(true);
    }
    let window = builder.build(&event_loop).unwrap();

    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::WindowExtWebSys;
        let query_string = web_sys::window().unwrap().location().search().unwrap();
        let level: log::Level = parse_url_query_string(&query_string, "RUST_LOG")
            .and_then(|x| x.parse().ok())
            .unwrap_or(log::Level::Error);
        console_log::init_with_level(level).expect("could not initialize logger");
        std::panic::set_hook(Box::new(console_error_panic_hook::hook));
        // On wasm, append the canvas to the document body
        web_sys::window()
            .and_then(|win| win.document())
            .and_then(|doc| doc.body())
            .and_then(|body| {
                body.append_child(&web_sys::Element::from(
                    window.canvas().expect("Couldn't get canvas"),
                ))
                .ok()
            })
            .expect("couldn't append canvas to document body");
    }

    #[cfg(target_arch = "wasm32")]
    let mut offscreen_canvas_setup: Option<OffscreenCanvasSetup> = None;
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::WindowExtWebSys;

        let query_string = web_sys::window().unwrap().location().search().unwrap();
        if let Some(offscreen_canvas_param) =
            parse_url_query_string(&query_string, "offscreen_canvas")
        {
            if FromStr::from_str(offscreen_canvas_param) == Ok(true) {
                log::info!("Creating OffscreenCanvasSetup");

                let offscreen_canvas =
                    OffscreenCanvas::new(1024, 768).expect("couldn't create OffscreenCanvas");

                let bitmap_renderer = window
                    .canvas()
                    .expect("Couldn't get html canvas")
                    .get_context("bitmaprenderer")
                    .expect("couldn't create ImageBitmapRenderingContext (Result)")
                    .expect("couldn't create ImageBitmapRenderingContext (Option)")
                    .dyn_into::<ImageBitmapRenderingContext>()
                    .expect("couldn't convert into ImageBitmapRenderingContext");

                offscreen_canvas_setup = Some(OffscreenCanvasSetup {
                    offscreen_canvas,
                    bitmap_renderer,
                })
            }
        }
    };

    log::info!("Initializing the surface...");

    let backends = wgpu::util::backend_bits_from_env().unwrap_or_default();
    let dx12_shader_compiler = wgpu::util::dx12_shader_compiler_from_env().unwrap_or_default();
    let gles_minor_version = wgpu::util::gles_minor_version_from_env().unwrap_or_default();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        flags: wgpu::InstanceFlags::from_build_config().with_env(),
        dx12_shader_compiler,
        gles_minor_version,
    });
    let (size, surface) = unsafe {
        let size = window.inner_size();

        #[cfg(any(not(target_arch = "wasm32"), target_os = "emscripten"))]
        let surface = instance.create_surface(&window).unwrap();
        #[cfg(all(target_arch = "wasm32", not(target_os = "emscripten")))]
        let surface = {
            if let Some(offscreen_canvas_setup) = &offscreen_canvas_setup {
                log::info!("Creating surface from OffscreenCanvas");
                instance.create_surface_from_offscreen_canvas(
                    offscreen_canvas_setup.offscreen_canvas.clone(),
                )
            } else {
                instance.create_surface(&window)
            }
        }
        .unwrap();

        (size, surface)
    };
    let adapter = wgpu::util::initialize_adapter_from_env_or_default(&instance, Some(&surface))
        .await
        .expect("No suitable GPU adapters found on the system!");

    #[cfg(not(target_arch = "wasm32"))]
    {
        let adapter_info = adapter.get_info();
        println!("Using {} ({:?})", adapter_info.name, adapter_info.backend);
    }

    let optional_features = E::optional_features();
    let required_features = E::required_features();
    let adapter_features = adapter.features();
    assert!(
        adapter_features.contains(required_features),
        "Adapter does not support required features for this example: {:?}",
        required_features - adapter_features
    );

    let required_downlevel_capabilities = E::required_downlevel_capabilities();
    let downlevel_capabilities = adapter.get_downlevel_capabilities();
    assert!(
        downlevel_capabilities.shader_model >= required_downlevel_capabilities.shader_model,
        "Adapter does not support the minimum shader model required to run this example: {:?}",
        required_downlevel_capabilities.shader_model
    );
    assert!(
        downlevel_capabilities
            .flags
            .contains(required_downlevel_capabilities.flags),
        "Adapter does not support the downlevel capabilities required to run this example: {:?}",
        required_downlevel_capabilities.flags - downlevel_capabilities.flags
    );

    // Make sure we use the texture resolution limits from the adapter, so we can support images the size of the surface.
    let needed_limits = E::required_limits().using_resolution(adapter.limits());

    let trace_dir = std::env::var("WGPU_TRACE");
    let (device, queue) = adapter
        .request_device(
            &wgpu::DeviceDescriptor {
                label: None,
                features: (optional_features & adapter_features) | required_features,
                limits: needed_limits,
            },
            trace_dir.ok().as_ref().map(std::path::Path::new),
        )
        .await
        .expect("Unable to find a suitable GPU adapter!");

    Setup {
        _window: window,
        event_loop,
        instance,
        size,
        surface,
        adapter,
        device,
        queue,
        #[cfg(target_arch = "wasm32")]
        offscreen_canvas_setup,
    }
}

fn start<E: Example>(
    #[cfg(not(target_arch = "wasm32"))] Setup {
        event_loop,
        instance,
        size,
        surface,
        adapter,
        device,
        queue,
        ..
    }: Setup,
    #[cfg(target_arch = "wasm32")] Setup {
        event_loop,
        instance,
        size,
        surface,
        adapter,
        device,
        queue,
        offscreen_canvas_setup,
        ..
    }: Setup,
) {
    let mut config = surface
        .get_default_config(&adapter, size.width, size.height)
        .expect("Surface isn't supported by the adapter.");
    let surface_view_format = config.format.add_srgb_suffix();
    config.view_formats.push(surface_view_format);
    surface.configure(&device, &config);

    log::info!("Initializing the example...");
    let mut example = E::init(&config, &adapter, &device, &queue);

    #[cfg(not(target_arch = "wasm32"))]
    let mut last_frame_inst = Instant::now();
    #[cfg(not(target_arch = "wasm32"))]
    let (mut frame_count, mut accum_time) = (0, 0.0);

    log::info!("Entering render loop...");
    event_loop
        .run(move |event, target| {
            let _ = (&instance, &adapter); // force ownership by the closure
            target.set_control_flow(ControlFlow::Poll);

            if cfg!(feature = "metal-auto-capture") {
                target.exit();
            };

            match event {
                event::Event::WindowEvent {
                    event: WindowEvent::Resized(size),
                    ..
                } => {
                    config.width = size.width.max(1);
                    config.height = size.height.max(1);
                    example.resize(&config, &device, &queue);
                    surface.configure(&device, &config);
                }
                event::Event::WindowEvent { event, .. } => match event {
                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                logical_key: Key::Named(NamedKey::Escape),
                                ..
                            },
                        ..
                    }
                    | WindowEvent::CloseRequested => {
                        target.exit();
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    WindowEvent::KeyboardInput {
                        event:
                            KeyEvent {
                                logical_key: Key::Character(s),
                                ..
                            },
                        ..
                    } if s == "r" => {
                        println!("{:#?}", instance.generate_report());
                    }
                    event::WindowEvent::RedrawRequested => {
                        #[cfg(not(target_arch = "wasm32"))]
                        {
                            accum_time += last_frame_inst.elapsed().as_secs_f32();
                            last_frame_inst = Instant::now();
                            frame_count += 1;
                            if frame_count == 100 {
                                println!(
                                    "Avg frame time {}ms",
                                    accum_time * 1000.0 / frame_count as f32
                                );
                                accum_time = 0.0;
                                frame_count = 0;
                            }
                        }

                        let frame = match surface.get_current_texture() {
                            Ok(frame) => frame,
                            Err(_) => {
                                surface.configure(&device, &config);
                                surface
                                    .get_current_texture()
                                    .expect("Failed to acquire next surface texture!")
                            }
                        };
                        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                            format: Some(surface_view_format),
                            ..wgpu::TextureViewDescriptor::default()
                        });

                        example.render(&view, &device, &queue);

                        frame.present();

                        #[cfg(target_arch = "wasm32")]
                        {
                            if let Some(offscreen_canvas_setup) = &offscreen_canvas_setup {
                                let image_bitmap = offscreen_canvas_setup
                                    .offscreen_canvas
                                    .transfer_to_image_bitmap()
                                    .expect("couldn't transfer offscreen canvas to image bitmap.");
                                offscreen_canvas_setup
                                    .bitmap_renderer
                                    .transfer_from_image_bitmap(&image_bitmap);

                                log::info!("Transferring OffscreenCanvas to ImageBitmapRenderer");
                            }
                        }
                    }
                    _ => example.update(event),
                },
                _ => {}
            }
        })
        .unwrap();
}

#[cfg(not(target_arch = "wasm32"))]
pub fn run<E: Example>(title: &str) {
    let setup = pollster::block_on(setup::<E>(title));
    start::<E>(setup);
}

#[cfg(target_arch = "wasm32")]
pub fn run<E: Example>(title: &str) {
    let title = title.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        let setup = setup::<E>(&title).await;
        let start_closure = Closure::once_into_js(move || start::<E>(setup));

        // make sure to handle JS exceptions thrown inside start.
        // Otherwise wasm_bindgen_futures Queue would break and never handle any tasks again.
        // This is required, because winit uses JS exception for control flow to escape from `run`.
        if let Err(error) = call_catch(&start_closure) {
            let is_control_flow_exception = error.dyn_ref::<js_sys::Error>().map_or(false, |e| {
                e.message().includes("Using exceptions for control flow", 0)
            });

            if !is_control_flow_exception {
                web_sys::console::error_1(&error);
            }
        }

        #[wasm_bindgen]
        extern "C" {
            #[wasm_bindgen(catch, js_namespace = Function, js_name = "prototype.call.call")]
            fn call_catch(this: &JsValue) -> Result<(), JsValue>;
        }
    });
}

#[cfg(target_arch = "wasm32")]
/// Parse the query string as returned by `web_sys::window()?.location().search()?` and get a
/// specific key out of it.
pub fn parse_url_query_string<'a>(query: &'a str, search_key: &str) -> Option<&'a str> {
    let query_string = query.strip_prefix('?')?;

    for pair in query_string.split('&') {
        let mut pair = pair.split('=');
        let key = pair.next()?;
        let value = pair.next()?;

        if key == search_key {
            return Some(value);
        }
    }

    None
}

pub use wgpu_test::image::ComparisonType;

#[derive(Clone)]
pub struct ExampleTestParams<E> {
    pub name: &'static str,
    // Path to the reference image, relative to the root of the repo.
    pub image_path: &'static str,
    pub width: u32,
    pub height: u32,
    pub optional_features: wgpu::Features,
    pub base_test_parameters: wgpu_test::TestParameters,
    /// Comparisons against FLIP statistics that determine if the test passes or fails.
    pub comparisons: &'static [ComparisonType],
    pub _phantom: std::marker::PhantomData<E>,
}

impl<E: Example + WasmNotSend + WasmNotSync> From<ExampleTestParams<E>> for GpuTestConfiguration {
    fn from(params: ExampleTestParams<E>) -> Self {
        GpuTestConfiguration::new()
            .name(params.name)
            .parameters({
                assert_eq!(params.width % 64, 0, "width needs to be aligned 64");

                let features = E::required_features() | params.optional_features;

                params.base_test_parameters.clone().features(features)
            })
            .run_async(move |ctx| async move {
                let dst_texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("destination"),
                    size: wgpu::Extent3d {
                        width: params.width,
                        height: params.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });

                let dst_view = dst_texture.create_view(&wgpu::TextureViewDescriptor::default());

                let dst_buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("image map buffer"),
                    size: params.width as u64 * params.height as u64 * 4,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });

                let mut example = E::init(
                    &wgpu::SurfaceConfiguration {
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        width: params.width,
                        height: params.height,
                        present_mode: wgpu::PresentMode::Fifo,
                        alpha_mode: wgpu::CompositeAlphaMode::Auto,
                        view_formats: vec![wgpu::TextureFormat::Rgba8UnormSrgb],
                    },
                    &ctx.adapter,
                    &ctx.device,
                    &ctx.queue,
                );

                example.render(&dst_view, &ctx.device, &ctx.queue);

                let mut cmd_buf = ctx
                    .device
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());

                cmd_buf.copy_texture_to_buffer(
                    wgpu::ImageCopyTexture {
                        texture: &dst_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::ImageCopyBuffer {
                        buffer: &dst_buffer,
                        layout: wgpu::ImageDataLayout {
                            offset: 0,
                            bytes_per_row: Some(params.width * 4),
                            rows_per_image: None,
                        },
                    },
                    wgpu::Extent3d {
                        width: params.width,
                        height: params.height,
                        depth_or_array_layers: 1,
                    },
                );

                ctx.queue.submit(Some(cmd_buf.finish()));

                let dst_buffer_slice = dst_buffer.slice(..);
                dst_buffer_slice.map_async(wgpu::MapMode::Read, |_| ());
                ctx.device.poll(wgpu::Maintain::Wait);
                let bytes = dst_buffer_slice.get_mapped_range().to_vec();

                wgpu_test::image::compare_image_output(
                    env!("CARGO_MANIFEST_DIR").to_string() + "/../../" + params.image_path,
                    &ctx.adapter_info,
                    params.width,
                    params.height,
                    &bytes,
                    params.comparisons,
                )
                .await;
            })
    }
}
