use bevy::prelude::*;
use godot::prelude::*;
use godot_bevy::prelude::*;

#[bevy_app]
fn build_app(app: &mut App) {
    app.add_plugins(GodotDefaultPlugins);
    app.add_systems(Update, hello_world_system);
}

fn hello_world_system(mut timer: Local<f32>, time: Res<Time>) {
    *timer += time.delta_secs();
    if *timer > 1.0 {
        *timer = 0.0;
        godot_print!("Hello from Bevy ECS!");
    }
}
