# Contrato de compatibilidade

## Plataforma

Uma janela nativa winit, uma superfície WebGPU e um isolate V8. A thread principal cuida da janela e da criação da superfície. Uma thread dedicada executa V8 com um executor Tokio de uma única thread, requisito das operações assíncronas do Deno. Uma fila limitada conecta as duas threads; decodificação de imagens e áudio usa workers separados. Os loops dormem quando não há trabalho e são acordados por eventos, timers ou operações assíncronas.

Os callbacks `Peregrust.onFrame` podem retornar uma Promise. A apresentação ocorre quando o trabalho do quadro termina. O host não inicia outro quadro enquanto o anterior está pendente. `requestAnimationFrame` também aceita callbacks assíncronos neste runtime; essa espera é uma extensão em relação ao comportamento do navegador. Não aguarde um próximo RAF dentro de um callback de quadro assíncrono, pois os quadros são serializados.

As dimensões de `canvas.width/height` são pixels do buffer de renderização. `window.innerWidth/innerHeight` e coordenadas do mouse são pixels lógicos. `devicePixelRatio` faz a conversão. Alterar o tamanho do canvas reconfigura a superfície sem redimensionar a janela nativa, permitindo o render scale do Three.js. O evento de resize informa ao jogo quando ele precisa recalcular sua resolução.

## Three.js

Use a versão fixada no `package-lock.json` e `three/webgpu`. O bundler redireciona imports `three` dos addons para a mesma implementação WebGPU, evitando instâncias duplicadas do Three. A cena de exemplo contém iluminação, textura PNG decodificada nativamente, GLB com animação e eventos de entrada.

Ao criar o renderer, passe `alpha: false`: a superfície da janela nativa suporta apresentação opaca. O valor padrão do Three pede alpha premultiplicado, que essa superfície rejeita na configuração WebGPU. Exemplo: `new THREE.WebGPURenderer({ canvas: Peregrust.canvas, alpha: false })`. O [exemplo de carga](../examples/stress.js) importa apenas `three/webgpu` e obtém o canvas nativo do global `Peregrust`.

- `WebGPURenderer`, geometrias, materiais compatíveis com WebGPU, instancing, animação e DataTexture usam o caminho real de GPU.
- Use `Peregrust.assets.read(path)` para dados binários e `Peregrust.assets.decodeImage(path)` para PNG/JPEG/WebP em RGBA8. `loadGLTF(path)`, exportado por `peregrust/three`, usa o GLTFLoader real e um adaptador de DataTexture para imagens embutidas ou externas. O exemplo carrega GLB com textura e animação. Compressão DRACO/KTX2/meshopt e buffers externos de `.gltf` não estão nesse adaptador.
- O DOM é restrito à janela, ao documento mínimo e ao canvas nativo. Não há HTML, layout CSS ou canvas 2D.
- Não há WebGL. `WebGLRenderer`, materiais GLSL exclusivos de WebGL e addons que dependam dele precisam ser adaptados. Materiais personalizados devem usar o caminho WebGPU/TSL do Three.
- `fetch` lê arquivos relativos à raiz do projeto, URLs `file:` confinadas a essa raiz, `blob:` e `data:`. `Response` oferece bytes, texto, JSON, Blob e ReadableStream para os loaders do Three.js. `Image`, `HTMLImageElement` e `createImageBitmap` decodificam imagens nativamente; `GPUQueue.copyExternalImageToTexture` transfere seus pixels para texturas RGBA/BGRA8. Consulte os limites explícitos na seção de arquivos.
- HTTP, WebAudio, WebXR, WebRTC, Web Workers e APIs Node.js não fazem parte deste contrato. Loaders que dependam dessas APIs precisam de adaptação; não assuma compatibilidade de qualquer addon apenas por ele importar Three.js.
- Captura de mouse, pointer lock, fullscreen, título e visibilidade do cursor são operações nativas; erros do sistema são expostos ao jogo. Capture e libere o mouse ao entrar/sair dos modos apropriados do jogo.
- `Peregrust.gamepads.poll()` e `navigator.getGamepads()` consultam controles nativos via Gilrs, com botões, eixos e desconexão. A inicialização é sob demanda. Não há rumble neste SDK.
- `Peregrust.audio.load(path)` fornece clips nativos Kira, com reprodução, pause/resume, stop, volume e loop; é uma API própria, sem WebAudio. WAV/OGG/MP3 são predecodificados. Limites: 16 MiB por arquivo, 128 MiB de cache decodificado, 128 clips e 64 vozes simultâneas. Libere clips e vozes quando não forem mais necessários. Streaming de música e rede ainda precisam de APIs específicas.

## Controle por IA

`--control SESSION` habilita um endpoint TCP local autenticado e grava o arquivo
de sessão. O cliente `peregrust ctl` retorna JSON. Os pedidos de inspeção e
entrada são processados nos limites de quadro, no thread do V8; o thread de
transporte não acessa objetos JavaScript. O registro `Peregrust.control` permite
conectar adaptadores de cena e providers de estado síncronos. O adaptador
`peregrust/inspect/three` é opcional. Consulte [o contrato de controle](agent-control.md),
incluindo semântica de timeout, quadros e captura offscreen.

## Persistência e extensões do jogo

`localStorage` persiste strings e `Peregrust.storage` persiste buffers binários, com backup e substituição atômica. O diretório padrão pertence ao usuário do sistema; `--app-id` mantém a identidade entre atualizações e `--storage-dir` permite um diretório explícito. Operações de escrita usam bloqueio entre processos. A importação opcional de saves Mystral é somente leitura e não modifica os arquivos antigos.

Um jogo com serviços Rust próprios pode ligar a crate Peregrust e fornecer uma fábrica de extensões por `run_with_options`. A fábrica roda na thread do V8, antes de carregar o módulo do jogo. O resultado continua sendo um executável desktop; o host Wuxia usa esse mecanismo para sua simulação Jianghu e memória compartilhada.

## Arquivos e recursos

Módulos e assets devem estar em `--root`. A resolução verifica caminhos canônicos, incluindo links simbólicos. Arquivos de assets têm limite de 128 MiB por leitura; imagens têm limite de 8192 pixels por dimensão e um orçamento de alocação para o decoder. Esses limites são explícitos; divida assets grandes em partes antes da carga. Não há streaming automático de mundos, mipmaps ou compressão de texturas.

Os limites não são uma sandbox contra alterações concorrentes no sistema de arquivos. O jogo deve ser confiável e o diretório da distribuição não deve ser modificável por terceiros durante a execução.

## Falhas

Erros de inicialização, rejeições de Promise não tratadas e falhas de callbacks encerram o processo com código diferente de zero. `Peregrust.exit(code)` solicita encerramento ordenado. O host libera o runtime/superfície antes dos handles da janela.

Resize e DPI reconfiguram a superfície; janelas sem área útil suspendem desenho. Falhas transitórias de aquisição/apresentação podem reconfigurar a superfície. Perda do dispositivo é exposta pela API WebGPU: o jogo precisa escolher entre reconstruir seus recursos ou encerrar com um diagnóstico. Recuperar uma superfície não recupera todos os recursos de um dispositivo perdido.

Use `--timeout` em testes automatizados para limitar o tempo total, inclusive execução JavaScript bloqueante. O limite de heap V8, quando configurado, não inclui VRAM, buffers externos, texturas e todos os recursos nativos. Esgotamento fatal de memória do V8 não é uma exceção JavaScript recuperável.
