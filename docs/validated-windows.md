# Primeiro marco: Three.js no executável desktop

Verificado localmente no Windows em 24/09/2026, com o executável de desenvolvimento `target/debug/peregrust.exe`. Estes resultados validam o primeiro marco de renderização; não certificam o jogo wuxia inteiro nem todas as plataformas desktop.

- 14 testes Rust passaram (carregador, caminhos, imagens, áudio e mapeamento de controles).
- 16 testes JavaScript passaram e o SDK passou pela checagem de tipos.
- 12 casos de integração passaram no processo nativo, incluindo TypeScript/imports, timers, assets, timeout de JavaScript travado, erros e pixels de WebGPU.
- O teste Three/WebGPU leu os pixels vermelho `[255,0,0,255]` e azul `[0,0,255,255]` do render target, além de pixels distintos de uma textura GLB decodificada nativamente. A apresentação na superfície também terminou com sucesso.
- `examples/hello-three.js`, que importa somente `three/webgpu` e usa `renderer.setAnimationLoop`, completou e apresentou 60 quadros.
- `examples/three-demo.js`, com iluminação, PNG e GLB animado, completou e apresentou 120 quadros.

As duas cenas foram executadas com limite de 60 FPS. São verificações funcionais de apresentação e ciclo de vida; os números não são um benchmark de capacidade para jogos pesados. As estatísticas locais estão em `artifacts/hello-three.json`, `artifacts/three-demo.json` e `artifacts/gpu-proof.json` (artefatos ignorados pelo Git).

O executável contém V8 e o bootstrap de APIs. O Node.js é usado para ferramentas de desenvolvimento/bundle e não para executar o jogo dentro do Peregrust. Importar o módulo JavaScript `peregrust` é opcional.

## Integração Wuxia no mesmo dia

O host `wuxia/native/peregrust-host` agora liga a simulação Jianghu ao mesmo
V8 e WebGPU do Peregrust. O binário integrado fica em
`wuxia/native/targets/peregrust/debug/peregrust.exe`. Ele executou a tela inicial
com 600 apresentações e saída 0, usando a RTX 3070 Ti Laptop GPU. A cena
`simulation-ui` também atingiu `ready`, com 130 quadros do jogo e 326
apresentações incluindo a tela de carregamento.

A cena procedural `world` passou após corrigir a exposição de `self.URL` usada
por imagens GLB: carregou sem erros de textura, chegou a `ready` em 83,8 s e
apresentou 340 quadros contando a carga. O fluxo normal `npm run dev` do Wuxia
também compilou, verificou o host e abriu o menu.

Os testes verificaram persistência entre processos, memória compartilhada,
comandos, projeções e captura/restauração de um checkpoint de 14.761.417 bytes.
Os 28 testes Rust incluem migração de fixtures de saves, acesso concorrente,
backup e publicação atômica de gravação/exclusão. Os saves antigos do usuário
não foram alterados. Os testes JavaScript e a checagem TypeScript também passam.

`fetch`, Blob URLs, `ImageBitmap` e transferência de imagem para texturas
WebGPU fazem parte do contrato atual. A validação de pixels inclui recorte,
inversão, dimensões do canvas e o carregador GLTF do Three.js.

Esses resultados usam um build de desenvolvimento. Não constituem benchmark
de jogos pesados, teste prolongado de campanha ou validação das demais GPUs e
sistemas. A inspeção por Computer Use foi interrompida por Esc; não houve
validação manual completa dos controles nesta etapa. Consulte também
`wuxia/docs/development/native-runtime.md` para o estado do jogo.
