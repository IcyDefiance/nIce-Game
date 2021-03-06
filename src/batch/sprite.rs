mod font;
mod shaders;
mod shared;
mod sprite;

pub use self::font::Font;
pub use self::shaders::SpriteBatchShaders;
pub use self::shared::SpriteBatchShared;
pub use self::sprite::Sprite;
use crate::{ ImageFramebuffer, ObjectId, RenderTarget, window::Window };
use std::sync::Arc;
use vulkano::{
	OomError,
	buffer::{ BufferUsage, ImmutableBuffer },
	command_buffer::{ AutoCommandBuffer, AutoCommandBufferBuilder, BuildError },
	descriptor::{ DescriptorSet, PipelineLayoutAbstract, descriptor_set::PersistentDescriptorSet },
	device::Queue,
	framebuffer::{ Framebuffer, FramebufferAbstract, FramebufferCreationError },
	image::ImageViewAccess,
	instance::QueueFamily,
	memory::DeviceMemoryAllocError,
	sync::GpuFuture,
};

pub struct SpriteBatch {
	shared: Arc<SpriteBatchShared>,
	sprites: Vec<Box<Drawable2D>>,
	framebuffers: Vec<ImageFramebuffer>,
	target_id: ObjectId,
	target_desc: Arc<DescriptorSet + Send + Sync + 'static>,
}
impl SpriteBatch {
	pub fn new(
		window: &Window,
		target: &RenderTarget,
		shared: Arc<SpriteBatchShared>
	) -> Result<(Self, impl GpuFuture), DeviceMemoryAllocError> {
		let dimensions = target.images()[0].dimensions();
		let (target_descs, future) =
			Self::make_target_desc(
				window.device().queue().clone(),
				shared.pipeline_sprite().clone(),
				dimensions.width(),
				dimensions.height()
			)?;

		let framebuffers =
			target.images().iter()
				.map(|image| {
					Framebuffer::start(shared.subpass().render_pass().clone())
						.add(image.clone())
						.and_then(|fb| fb.build())
						.map(|fb| ImageFramebuffer::new(Arc::downgrade(&image), Arc::new(fb)))
						.map_err(|err| match err {
							FramebufferCreationError::OomError(err) => err,
							err => unreachable!("{:?}", err),
						})
				})
				.collect::<Result<Vec<_>, _>>()?;

		Ok((
			Self {
				shared: shared,
				sprites: vec![],
				framebuffers: framebuffers,
				target_id: target.id_root().make_id(),
				target_desc: target_descs,
			},
			future
		))
	}

	pub fn add_sprite(&mut self, sprite: Box<Drawable2D>) {
		self.sprites.push(sprite);
	}

	fn make_target_desc(
		queue: Arc<Queue>,
		pipeline: impl PipelineLayoutAbstract + Send + Sync + 'static,
		width: u32,
		height: u32
	) -> Result<(Arc<DescriptorSet + Send + Sync + 'static>, impl GpuFuture), DeviceMemoryAllocError> {
		let (target_size, future) = ImmutableBuffer::from_data([width, height], BufferUsage::uniform_buffer(), queue)?;

		Ok((
			Arc::new(
				PersistentDescriptorSet::start(pipeline, 0)
					.add_buffer(target_size.clone())
					.unwrap()
					.build()
					.unwrap()
			),
			future
		))
	}

	pub fn commands(
		&mut self,
		window: &Window,
		target: &RenderTarget,
		image_num: usize,
	) -> Result<(AutoCommandBuffer, Option<impl GpuFuture>), DeviceMemoryAllocError> {
		assert!(self.target_id.is_child_of(target.id_root()));

		let framebuffer = self.framebuffers[image_num].image
			.upgrade()
			.iter()
			.filter(|old_image| Arc::ptr_eq(&target.images()[image_num], &old_image))
			.next()
			.map(|_| self.framebuffers[image_num].framebuffer.clone());
		let (framebuffer, future) =
			if let Some(framebuffer) = framebuffer {
				(framebuffer, None)
			} else {
				let framebuffer = Framebuffer::start(self.shared.subpass().render_pass().clone())
					.add(target.images()[image_num].clone())
					.and_then(|fb| fb.build())
					.map(|fb| Arc::new(fb))
					.map_err(|err| {
						match err { FramebufferCreationError::OomError(err) => err, err => unreachable!("{:?}", err) }
					})?;
				self.framebuffers[image_num] =
					ImageFramebuffer::new(Arc::downgrade(&target.images()[image_num]), framebuffer.clone());

				let (target_desc, future) =
					Self::make_target_desc(
						window.device().queue().clone(),
						self.shared.pipeline_sprite().clone(),
						framebuffer.width(),
						framebuffer.height()
					)?;

				self.target_desc = target_desc;

				(framebuffer as _, Some(future))
			};

		let dimensions = [framebuffer.width() as f32, framebuffer.height() as f32];

		let mut command_buffer =
			AutoCommandBufferBuilder::primary_one_time_submit(self.shared.shaders().device().clone(), window.device().queue().family())?
				.begin_render_pass(framebuffer, true, vec![[0.1, 0.1, 0.1, 1.0].into()])
				.unwrap();

		for sprite in &mut self.sprites {
			command_buffer =
				unsafe {
					command_buffer
						.execute_commands(
							sprite.make_commands(&self.shared, &self.target_desc, window.device().queue().family(), dimensions)?
						)
						.unwrap()
				};
		}

		Ok((
			command_buffer.end_render_pass().unwrap()
				.build()
				.map_err(|err| match err { BuildError::OomError(err) => err, err => unreachable!("{}", err) })?,
			future
		))
	}
}

pub trait Drawable2D {
	fn make_commands(
		&mut self,
		shared: &SpriteBatchShared,
		target_desc: &Arc<DescriptorSet + Send + Sync + 'static>,
		queue_family: QueueFamily,
		dimensions: [f32; 2],
	) -> Result<AutoCommandBuffer, OomError>;
}
