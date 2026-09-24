# Validação de release

Nenhum resultado de uma cena simples estabelece capacidade para todos os jogos pesados. Registre a versão do runtime, commit, Cargo.lock, package-lock.json, sistema operacional, GPU, driver e resolução para cada execução.

## Gate automatizado

1. Formatação, Clippy sem warnings e testes Rust.
2. Testes do scheduler/eventos/assets e checagem de tipos do SDK.
3. Compilação release com lockfiles.
4. Integração nativa: TypeScript com imports e top-level await, timers, erros de inicialização/quadro/Promise, confinamento de arquivos e prova de pixels produzidos pelo Three/WebGPU.
5. Cena com assets reais e teste de carga com estatísticas JSON. Saída diferente de zero deve falhar o pipeline.

O workflow de CI é uma configuração executável; os resultados locais não comprovam que os runners remotos passaram. Uma versão só deve ser promovida após esses jobs terminarem com sucesso no repositório de distribuição.

## Gate por plataforma e jogo

- Windows: testar D3D12 e Vulkan nas GPUs NVIDIA, AMD e Intel que o produto declara suportar.
- Linux: testar os drivers e sistemas de janelas suportados (X11/Wayland), incluindo uma sessão real.
- macOS: testar Metal e as arquiteturas efetivamente distribuídas.
- Mover a janela entre monitores com DPI e frequência diferentes, redimensionar repetidamente, minimizar/restaurar, fullscreen, foco e captura do mouse.
- Jogar por sessões prolongadas com assets representativos; observar RAM, VRAM, GC, latência de entrada e p95/p99 de quadro. Repetir ciclos de carregamento/descarregamento de cenas para detectar recursos retidos.
- Testar desconexão de dispositivos, suspensão/retomada do sistema e perda/recriação do dispositivo gráfico conforme a política do jogo.
- Verificar o pacote em uma máquina limpa sem Node.js, Deno ou Rust instalados; registrar dependências do sistema, avisos de licença, assinatura de código e hashes do artefato.

Não desabilite validação WebGPU para esconder erros. Atualizações dos crates Deno e wgpu devem passar novamente pela suíte completa; não misture versões incompatíveis de `deno_core`, `deno_web`, `deno_webidl` e `deno_webgpu`.
