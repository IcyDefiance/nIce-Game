pub use winit::{ CursorState, Event, MouseButton, WindowEvent, WindowId };

use { Context, ObjectIdRoot, RenderTarget };
use cgmath::Vector2;
use std::{ collections::HashMap, iter::Iterator, sync::{ Arc, atomic::{ AtomicBool, Ordering } }};
use vulkano::{
	device::{ Device, DeviceExtensions, Queue },
	format::Format,
	image::ImageViewAccess,
	instance::{ Features, PhysicalDevice },
	memory::DeviceMemoryAllocError,
	swapchain::{
		acquire_next_image,
		AcquireError,
		PresentMode,
		Surface,
		SurfaceTransform,
		Swapchain,
		SwapchainCreationError
	},
	sync::{ FlushError, GpuFuture },
};
use vulkano_win::VkSurfaceBuild;
use winit;

pub struct EventsLoop {
	events: winit::EventsLoop,
	resized: HashMap<WindowId, Arc<AtomicBool>>,
}
impl EventsLoop {
	pub fn new() -> Self {
		Self { events: winit::EventsLoop::new(), resized: HashMap::new() }
	}

	pub fn poll_events(&mut self, mut callback: impl FnMut(Event)) {
		let resized = &mut self.resized;
		self.events.poll_events(|event| {
			match event {
				Event::WindowEvent { event: WindowEvent::Closed, window_id } => {
					resized.remove(&window_id);
				},
				Event::WindowEvent { event: WindowEvent::Resized(_, _), window_id } => {
					resized[&window_id].store(true, Ordering::Relaxed);
				},
				_ => (),
			}

			callback(event);
		});
	}
}

pub struct Window {
	surface: Arc<Surface<winit::Window>>,
	device: Arc<Device>,
	queue: Arc<Queue>,
	swapchain: Arc<Swapchain<winit::Window>>,
	images: Vec<Arc<ImageViewAccess + Send + Sync + 'static>>,
	previous_frame_end: Option<Box<GpuFuture>>,
	resized: Arc<AtomicBool>,
	id_root: ObjectIdRoot,
}
impl Window {
	pub fn new<T: Into<String>>(ctx: &Context, events: &mut EventsLoop, title: T) -> Self {
		let pdevice = PhysicalDevice::enumerate(&ctx.instance).next().expect("no device available");
		info!("Using device: {} ({:?})", pdevice.name(), pdevice.ty());

		let surface = winit::WindowBuilder::new()
			.with_title(title)
			.build_vk_surface(&events.events, ctx.instance.clone())
			.expect("failed to create window");

		let qfam = pdevice.queue_families()
			.find(|&q| q.supports_graphics() && surface.is_supported(q).unwrap())
			.expect("failed to find a graphical queue family");

		let (device, mut queues) =
			Device::new(
				pdevice,
				&Features::none(),
				&DeviceExtensions { khr_swapchain: true, .. DeviceExtensions::none() },
				[(qfam, 1.0)].iter().cloned()
			)
			.expect("failed to create device");
		let queue = queues.next().unwrap();

		let (swapchain, images) = {
			let caps = surface.capabilities(pdevice).expect("failed to get surface capabilities");
			Swapchain::new(
				device.clone(),
				surface.clone(),
				caps.min_image_count,
				Format::B8G8R8A8Srgb,
				caps.current_extent.unwrap_or(surface.window().get_inner_size().map(|(x, y)| [x, y]).unwrap()),
				1,
				caps.supported_usage_flags,
				&queue,
				SurfaceTransform::Identity,
				caps.supported_composite_alpha.iter().next().unwrap(),
				PresentMode::Fifo,
				true,
				None
			).expect("failed to create swapchain")
		};
		let images = images.into_iter().map(|x| x as _).collect();

		let resized = Arc::<AtomicBool>::default();
		events.resized.insert(surface.window().id(), resized.clone());

		Self {
			surface: surface,
			device: device,
			queue: queue,
			swapchain: swapchain,
			images: images,
			previous_frame_end: None,
			resized: resized,
			id_root: ObjectIdRoot::new(),
		}
	}

	pub fn join_future(&mut self, future: impl GpuFuture + 'static) {
		if let Some(previous_frame_end) = self.previous_frame_end.take() {
			self.previous_frame_end = Some(Box::new(previous_frame_end.join(future)));
		} else {
			self.previous_frame_end = Some(Box::new(future));
		}
	}

	pub fn present<F>(
		&mut self,
		get_commands: impl FnOnce(&mut Self, usize, Box<GpuFuture>) -> F
	) -> Result<(), DeviceMemoryAllocError>
	where
		F: GpuFuture + 'static
	{
		if self.resized.swap(false, Ordering::Relaxed) {
			let dimensions = self.surface.capabilities(self.device.physical_device())
				.expect("failed to get surface capabilities")
				.current_extent
				.unwrap_or(self.surface.window().get_inner_size().map(|(x, y)| [x, y]).unwrap());

			let (swapchain, images) =
				match self.swapchain.recreate_with_dimension(dimensions) {
					Ok(ret) => ret,
					Err(SwapchainCreationError::UnsupportedDimensions) => {
						self.resized.store(true, Ordering::Relaxed);
						return Ok(());
					},
					Err(err) => unreachable!(err),
				};

			self.swapchain = swapchain;
			self.images = images.into_iter().map(|x| x as _).collect();
		}

		let (image_num, acquire_future) =
			match acquire_next_image(self.swapchain.clone(), None) {
				Ok(val) => val,
				Err(AcquireError::OutOfDate) => {
					self.resized.store(true, Ordering::Relaxed);
					return Ok(());
				},
				Err(err) => unreachable!(err)
			};

		let mut future: Box<GpuFuture> =
			if let Some(mut future) = self.previous_frame_end.take() {
				future.cleanup_finished();
				Box::new(future.join(acquire_future))
			} else {
				Box::new(acquire_future)
			};
		future = Box::new(get_commands(self, image_num, future));
		let future = future.then_swapchain_present(self.queue.clone(), self.swapchain.clone(), image_num)
			.then_signal_fence_and_flush();
		self.previous_frame_end =
			match future {
				Ok(future) => Some(Box::new(future)),
				Err(FlushError::OutOfDate) => {
					self.resized.store(true, Ordering::Relaxed);
					return Ok(());
				},
				Err(err) => unreachable!(err),
			};

		Ok(())
	}

	pub fn get_inner_size(&self) -> Option<Vector2<u32>> {
		self.surface.window().get_inner_size().map(|size| size.into())
	}

	pub fn set_cursor_position(&self, pos: Vector2<i32>) -> Result<(), ()> {
		self.surface.window().set_cursor_position(pos.x, pos.y)
	}

	pub fn set_cursor_state(&self, state: CursorState) -> Result<(), String> {
		self.surface.window().set_cursor_state(state)
	}

	pub(super) fn device(&self) -> &Arc<Device> {
		&self.device
	}

	pub fn queue(&self) -> &Arc<Queue> {
		&self.queue
	}
}
impl RenderTarget for Window {
	fn format(&self) -> Format {
		self.swapchain.format()
	}

	fn id_root(&self) -> &ObjectIdRoot {
		&self.id_root
	}

	fn images(&self) -> &[Arc<ImageViewAccess + Send + Sync + 'static>] {
		&self.images
	}
}
