struct PrepassOutput {
    @builtin(position) @invariant position: vec4<f32>,
    @if(VERTEX_UVS_A)
    @location(0) uv: vec2<f32>,
    @if(VERTEX_UVS_B)
    @location(1) uv_b: vec2<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS)
    @location(2) world_normal: vec3<f32>,
    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS && VERTEX_TANGENTS)
    @location(3) world_tangent: vec4<f32>,
    @location(4) world_position: vec4<f32>,
    @if(MOTION_VECTOR_PREPASS)
    @location(5) previous_world_position: vec4<f32>,
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    @location(6) unclipped_depth: f32,
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    @location(7) @interpolate(flat) instance_index: u32,
    @if(VERTEX_COLORS)
    @location(8) color: vec4<f32>,
    @if(VISIBILITY_RANGE_DITHER)
    @location(9) @interpolate(flat) visibility_range_dither: i32,
}
