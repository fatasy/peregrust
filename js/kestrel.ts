// Classic-script bootstrap. Kept valid TypeScript and JavaScript so the host
// can load it without a separate transpilation step.
((root) => {
  const ops = root.Deno.core.ops;
  Object.defineProperty(root.Peregrust, 'kestrel', {
    configurable: false,
    enumerable: true,
    value: Object.freeze({
      abiVersion: 8,
      createRenderer: ops.op_kestrel_create_renderer,
      createGeometry: ops.op_kestrel_create_geometry,
      createShaderGeometry: ops.op_kestrel_create_shader_geometry,
      createTexture: ops.op_kestrel_create_texture,
      createCompressedTextureArray: ops.op_kestrel_create_compressed_texture_array,
      createMaterial: ops.op_kestrel_create_material,
      createShaderProgram: ops.op_kestrel_create_shader_program,
      createShaderMaterial: ops.op_kestrel_create_shader_material,
      createMesh: ops.op_kestrel_create_mesh,
      render: ops.op_kestrel_render,
      destroy: ops.op_kestrel_destroy,
    }),
  });
})(globalThis);
