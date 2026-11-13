#[cfg(feature = "pipeline")]
mod actors;
#[cfg(feature = "pipeline")]
mod anim;
#[cfg(feature = "pipeline")]
mod audio;
#[cfg(feature = "pipeline")]
pub mod career;
#[cfg(feature = "pipeline")]
pub mod content;
pub use ssx_rules::data_roots;
mod disc;
pub mod display;
#[cfg(feature = "pipeline")]
mod effects;
#[cfg(feature = "pipeline")]
mod landscape;
pub mod port_options;
pub mod settings_file;
#[cfg(feature = "pipeline")]
mod soundtrack;
#[cfg(feature = "pipeline")]
mod track;
#[cfg(feature = "pipeline")]
mod vaults;

#[cfg(feature = "pipeline")]
pub use actors::{character, ghosts, helicopter, nis};
#[cfg(feature = "pipeline")]
pub use anim::{
    anim_clip, anim_db, anim_eval, anim_graph, anim_leaf, anim_pose, anim_rigops, anim_space, rig_anim, rigamate, tags,
};
#[cfg(feature = "pipeline")]
pub use audio::{
    sfx, sfx_aems, sfx_avalanche, sfx_bus, sfx_dsp, sfx_dsp_remix, sfx_game, sfx_mixer, sfx_override, sfx_tuna,
};
#[cfg(feature = "pipeline")]
pub use disc::folder;
pub use disc::{exe, setup, subset};
#[cfg(feature = "pipeline")]
pub use effects::{light_script, particle_script, particles, postfx, postfx_overlay, track_particles};
#[cfg(feature = "pipeline")]
pub use landscape::{subdiv, terrain, terrain_material, terrain_vertex};
#[cfg(feature = "pipeline")]
pub use soundtrack::{
    music, music_script, musicbox, pathfinder, pathfinder_events, remix, remix_runtime, score, track_override,
};
#[cfg(feature = "pipeline")]
pub use track::{
    avalanche_entities, object_collision, object_mesh, texture, track_lighting, track_lights, track_objects,
};
#[cfg(feature = "pipeline")]
pub use vaults::{vault, vault_db};
