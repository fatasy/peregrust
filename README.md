# Peregrust

Runtime desktop para jogos em TypeScript/JavaScript, com **Rust + winit + V8 + WebGPU nativo**. O caminho de renderização usa `deno_webgpu`/`wgpu-core`; não existe Chromium ou servidor web no executável.

O Three.js usa `WebGPURenderer` de `three/webgpu`. O host e o JavaScript compartilham a mesma instância WebGPU e a mesma superfície nativa.

**A entrega principal é `peregrust.exe` (Windows) ou `peregrust` (Linux/macOS): um executável que abre o jogo.** V8 e as APIs nativas são incorporados ao binário. A pasta `js/` contém o bootstrap incorporado e helpers opcionais; o jogo não precisa importar uma biblioteca JavaScript Peregrust para executar. Veja [hello-three.js](examples/hello-three.js), que importa apenas Three.js e usa `renderer.setAnimationLoop`.

```powershell
peregrust.exe main.js --root C:\jogos\meu-jogo
```

Para Three.js, configure `new THREE.WebGPURenderer({ canvas: Peregrust.canvas, alpha: false })`. A superfície nativa aceita apresentação opaca; o valor padrão de alpha do Three solicita um modo premultiplicado que ela rejeita.

## Executar

Pré-requisitos de desenvolvimento: Rust conforme `rust-toolchain.toml`, Node.js 22 ou superior e toolchain C/C++ da plataforma. No Windows, instale Visual Studio Build Tools com **Desktop development with C++** e Windows SDK. O aplicativo compilado não depende de Node.js nem da instalação do Deno.

```powershell
npm ci
npm run build:demo
cargo build --release --locked
.\target\release\peregrust.exe dist/three-demo.js --root .
```

Neste workspace Windows, `./scripts/cargo.ps1` também reconhece o compilador portátil opcional em `.tools/msvc`. Ele não instala dependências e não é necessário em uma instalação padrão.

Linux/macOS:

```sh
npm ci
npm run build:demo
cargo build --release --locked
./target/release/peregrust dist/three-demo.js --root .
```

No Linux, o build exige as bibliotecas de desenvolvimento do sistema de janelas (por exemplo, `libxkbcommon-dev` e `libwayland-dev` no Ubuntu). A execução exige um driver gráfico compatível com o backend WebGPU disponível na plataforma.

## Seu jogo

```ts
const adapter = await navigator.gpu.requestAdapter({ powerPreference: 'high-performance' });
if (!adapter) throw new Error('WebGPU indisponível');
const device = await adapter.requestDevice();
const context = Peregrust.canvas.getContext('webgpu');
context.configure({ device, format: navigator.gpu.getPreferredCanvasFormat(), alphaMode: 'opaque' });

Peregrust.onFrame(() => {
  const encoder = device.createCommandEncoder();
  const pass = encoder.beginRenderPass({
    colorAttachments: [{
      view: context.getCurrentTexture().createView(),
      clearValue: { r: 0.03, g: 0.08, b: 0.15, a: 1 },
      loadOp: 'clear', storeOp: 'store',
    }],
  });
  pass.end();
  device.queue.submit([encoder.finish()]);
});
```

Salve como `game.ts` e execute `peregrust game.ts --root .`. O runtime transpila TypeScript ao carregar módulos; a checagem de tipos acontece no seu pipeline, não durante o jogo. Imports locais, JSON com `with { type: 'json' }` e import maps locais são suportados. Para dependências npm, gere um bundle ESM:

```sh
node scripts/build.mjs caminho/game.ts dist/game.js
```

Veja [o exemplo Three.js](examples/three-demo.js), [as declarações do SDK](types/index.d.ts) e [o contrato de compatibilidade](docs/compatibility.md).

Para extensões Rust incorporadas ao jogo, veja [armazenamento nativo e transferência de buffers](docs/native-storage.md).

## Controle por IA

Inicie um jogo com `--control artifacts/game-session.json` para habilitar uma
sessão persistente. Em outro processo, `peregrust ctl --session
artifacts/game-session.json control.describe` descobre as operações disponíveis.
A CLI retorna JSON e permite consultar/alterar objetos, enviar teclado/mouse,
ler estado registrado pelo jogo e capturar imagens da cena Three.js.

O adaptador opcional `attachThree`, de `peregrust/inspect/three`, conecta a cena,
câmera e renderer à API. `input.key` mantém uma tecla por uma quantidade definida
de quadros e pode retornar estado e imagem juntos. Veja [o guia de controle por
IA](docs/agent-control.md) e [o exemplo executável](examples/control-demo.js).
MCP pode usar essa mesma API em uma próxima etapa; esta entrega oferece a CLI.

## Verificação

```sh
npm test
npm run typecheck
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
cargo build --locked
npm run test:integration
npm run test:control
```

Os testes de integração exigem uma sessão gráfica e GPU/driver WebGPU; não passam por simulação. Há verificações de pixels via leitura de textura, inicialização assíncrona, erros JavaScript e carregamento de assets. O CI em Linux usa uma sessão Xvfb e Vulkan por software para a parte automatizada; isso não substitui testes em GPUs reais.

Para medir sua cena:

```powershell
npm run build:stress
.\target\release\peregrust.exe dist/stress.js --frames 600 --timeout 120 --stats artifacts/stress.json -- --objects 10000
```

Crie o diretório de saída antes de usar `--stats`. Os tempos representam intervalos entre quadros concluídos pelo host, incluindo sincronização e apresentação. Não são timestamps de GPU nem uma comparação entre Dawn e wgpu. Use um build release, a mesma cena, resolução e driver ao comparar resultados.

`--frames` conta callbacks concluídos, incluindo quadros sem desenho durante a carga. Para exigir apresentação real na janela, use `--presented-frames`; o campo `presentedFrames` das estatísticas registra esse total. Combine esses limites com `--timeout` em testes automatizados.

## Distribuição e operação

O jogo pode ser distribuído com o executável, o bundle ESM e os assets. `--root` define o diretório do projeto; caminhos de assets e módulos ficam dentro dele. `--` separa argumentos do runtime e do jogo. Consulte `peregrust --help` para janela, fullscreen, limite de quadros, timeout e estatísticas.

O runtime executa **jogos locais confiáveis**. O confinamento de caminhos reduz leituras acidentais fora do projeto, mas não constitui uma fronteira de segurança para código hostil. Não execute mods ou downloads desconhecidos sem isolamento externo.

Antes de distribuir comercialmente, execute os testes e a matriz descrita em [validação de release](docs/release-validation.md), incluindo as GPUs e os sistemas realmente suportados pelo jogo. A implementação fornece mecanismos e testes; não equivale a uma certificação de todos os drivers, jogos ou sistemas desktop.

Código do projeto sob MIT. As dependências mantêm suas próprias licenças; preserve seus avisos ao distribuir.
