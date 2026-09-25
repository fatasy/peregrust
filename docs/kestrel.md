# Kestrel no Peregrust

O Peregrust hospeda a [engine Kestrel](../../kestrel/core). `src/kestrel/mod.rs` é somente a ponte de operações: recebe `GPUDevice` e `GPUTexture` do WebGPU do Peregrust, mantém o objeto cppgc do dispositivo vivo, encaminha os identificadores e o `wgpu-core::Global` ao crate e traduz erros para os error scopes do mesmo dispositivo. Os buffers, texturas, pipelines, passes, culling, draw e submit pertencem ao crate Rust `kestrel-core`. Nenhum segundo dispositivo é criado.

O bootstrap `js/kestrel.ts` publica `Peregrust.kestrel` ABI 8. Os recursos usam identificadores por renderer, não reutilizados. `destroy` usa kind 0 renderer, 1 geometry, 2 material, 3 mesh, 4 texture e 5 shader program. Recursos ainda referenciados são recusados. O host descarta o renderer antes de liberar a raiz cppgc do dispositivo.

## Operações nativas

| Operação | Contrato |
| --- | --- |
| `createRenderer` | `GPUDevice`, formato de cor, `Uint32Array [samples(1/4), hdr(0/1)]`; MSAA 4 exige HDR. |
| `createGeometry` | Layout básico: posição3, normal3, uv2, cor4; índices u32 e AABB min3/max3. |
| `createShaderProgram` | JSON de layout/estado e fontes WGSL independentes para vértice e fragmento; compila e retém pipeline no registro. Fontes iguais compartilham um módulo GPU. |
| `createShaderGeometry` | Programa, vértices no stride declarado, índices u32, AABB. Posição xyz ocupa os primeiros três floats. |
| `createMaterial` | Perfil básico Lambert/Standard com parâmetros e uma textura de cor opcional. |
| `createShaderMaterial` | Programa, uniforms concatenados na ordem declarada e handles de textura na ordem declarada. |
| `createTexture` | Descriptor de 12 u32 e bytes RGBA8/R32F/imagem codificada ou RGBA8 de canais independentes; `info` recebe largura, altura, mips. |
| `createCompressedTextureArray` | Descriptor `[format,size,layers,mips,anisotropy]` e bytes BC mip-major, layer-major. Formatos 0 BC1-sRGB, 1 BC4, 2 BC5; exige `texture-compression-bc` no dispositivo. |
| `createMesh` | Geometria, material, N matrizes de 16 floats e N cores de instância de 4 floats. |
| `render` | Uma chamada por frame com target, frame de 116 floats, pares sujos e matrizes alteradas; devolve quatro contadores. |

O descriptor de programa declara `vertex:{strideFloats,attributes:[{shaderLocation,offsetFloats,size}]}`, `vertexEntry`, `fragmentEntry`, `uniforms:[{binding,floatCount}]`, `textures:[{binding,samplerBinding?,kind}]`, `side`, `depthWrite`, `depthCompare`, `blend`, `sceneColor` e `frustumCulled`. Texturas aceitam `kind` `float`, `unfilterable-float` ou `array`; o sampler é opcional para shaders que usam `textureLoad`. O programa e cada material retêm seus layouts e buffers. `sceneColor` usa group 3 fixo: cor opaca HDR em binding 0, sampler linear em 1, profundidade 1× em 2 e profundidade MSAA 4× em 3. Exige HDR, profundidade somente de leitura suportada e `depthWrite:false`. O passe posterior preserva o teste de profundidade por amostra e usa a imagem opaca imutável.

`frustumCulled:false` desenha todas as instâncias, adequado para vertex shaders que deslocam a posição. `true` assume que a posição declarada e o AABB fornecido limitam conservadoramente a saída do shader. AABBs precisam conter todas as posições de entrada; matrizes devem ser afins sem reflexão. O renderer registra recursos uma vez, compacta instâncias visíveis em Rust e desenha por mesh registrado, sem chamada TypeScript por draw.

O frame tem viewProjection16, cameraPosition4, lightDirection4, lightColor4, ambient4, hemisphereSky4, hemisphereGround4, backgroundLinearRGBA4, `[exposure,toneMapping,outputColorSpace,0]`, inverseProjection16, cameraWorld16, `[timeSeconds,near,far,0]`, projectionMatrix16 e matrixWorldInverse16. `near` deve ser não negativo e `far > near`. Os contadores são `[draws,visible,culled,uploads]`; draw de saída HDR é contado. O frame estável evita upload de dados e reconstrução de pipelines. A correção visual e a meta de 2× exigem a validação do jogo completo, registrada no projeto Wuxia.

Os shaders de iluminação básica e saída de cor residem em `kestrel/core/src`; o ajuste ACES segue Stephen Hill/BakingLab sob [MIT](../../kestrel/core/BakingLab-MIT.txt). Os shaders e materiais de jogos residem nos respectivos projetos consumidores.
