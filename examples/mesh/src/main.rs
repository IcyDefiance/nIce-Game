extern crate futures;
extern crate nice_game;

use nice_game::{
	Context,
	GpuFuture,
	RenderTarget,
	Version,
	mesh::{ Mesh, MeshBatch, MeshBatchShaders, MeshBatchShared, MeshVertex },
	window::{ Event, EventsLoop, Window, WindowEvent },
};

fn main() {
	let mut events = EventsLoop::new();

	let mut window =
		Window::new(
			&Context::new(
				Some("Triangle Example"),
				Some(Version {
					major: env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
					minor: env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
					patch: env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
				}),
			).unwrap(),
			&mut events,
			"nIce Game"
		);

	let mesh_batch_shared = MeshBatchShared::new(MeshBatchShaders::new(&mut window).unwrap(), window.format());

	let (mesh, mesh_future) = Mesh::new(
		&mut window,
		vec![
			MeshVertex { position: [0.0, 0.0] },
			MeshVertex { position: [1.0, 0.0] },
			MeshVertex { position: [0.0, 1.0] },
			MeshVertex { position: [0.0, 1.0] },
			MeshVertex { position: [1.0, 0.0] },
			MeshVertex { position: [1.0, 1.0] },
		].into_iter(),
		[10.0, 10.0]
	).unwrap();

	let mut mesh_batch = MeshBatch::new(&mut window, mesh_batch_shared).unwrap();
	mesh_batch.add_mesh(mesh);

	window.join_future(mesh_future);

	loop {
		let mut done = false;
		events.poll_events(|event| match event {
			Event::WindowEvent { event: WindowEvent::Closed, .. } => done = true,
			_ => (),
		});

		if done {
			break;
		}

		window
			.present(|window, image_num, future| {
				future.then_execute(window.queue().clone(), mesh_batch.commands(window, window, image_num).unwrap())
					.unwrap()
			})
			.unwrap();
	}
}
