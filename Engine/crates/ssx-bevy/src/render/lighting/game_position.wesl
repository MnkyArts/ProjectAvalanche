import bevy_pbr::render::view_transformations::position_world_to_clip;

struct Placed {
    world_position: vec3<f32>,
    world_normal: vec3<f32>,
    clip_position: vec4<f32>,
}

fn place(world_position: vec3<f32>, world_normal: vec3<f32>) -> Placed {
    return Placed(world_position, world_normal, position_world_to_clip(world_position));
}

// PORT(choice): the families that write the level camera's depth prepass (the terrain, the disc's opaque and mask
// objects) declare the clip position `@invariant` in their main pass's vertex output (`MainOutput`: `game_vertex.wesl`
// with SSX_INVARIANT_POSITION, `render::gbuffer::invariant_main_pass`) and their prepasses' (`prepass_output.wesl`):
// WGSL makes two entry points' invariant positions equal where their data and control flow match, and `place`
// (`terrain_ripple::place` under SSX_TERRAIN_RIPPLE, after Bevy 0.20's `skin_model` / `get_world_from_local` and
// `mesh_position_local_to_world` on the same vertex) gives both passes one flow. naga 30 emits the SPIR-V decoration
// `Invariant` on the `Position` output (back/spv/writer.rs) and HLSL `precise` on the `SV_Position` member
// (back/hlsl/writer.rs). So the prepass writes the depth the main pass tests (`GreaterEqual`) on any driver, not only
// where two compilations happen to agree: Bevy's own outputs (`forward_io::VertexOutput`, `prepass::io::VertexOutput`)
// are not invariant. Only those families: with it on every game material's main pass the image gate (Mesa, Intel) moved
// the riders' and the ghosts' pixels (465-3 509 a view), with it on these families only 0 px; the characters, the
// foreground and the ghosts write no prepass depth. Members and locations are Bevy's (the fragment shaders take Bevy's
// structs; tests/wesl_compose.rs compares the two for every def set). The console drew no depth prepass: the port's
// own, `render::post::topogrid`.
struct MainOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @if(VERTEX_UVS_A)
    @location(2) uv: vec2<f32>,
    @if(VERTEX_UVS_B)
    @location(3) uv_b: vec2<f32>,
    @if(VERTEX_TANGENTS)
    @location(4) world_tangent: vec4<f32>,
    @if(VERTEX_COLORS)
    @location(5) color: vec4<f32>,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    @location(6) @interpolate(flat) instance_index: u32,
    @if(VISIBILITY_RANGE_DITHER)
    @location(7) @interpolate(flat) visibility_range_dither: i32,
}
