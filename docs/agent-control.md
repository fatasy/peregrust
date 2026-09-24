# Controle do runtime por IA

O Peregrust oferece uma sessão persistente de controle local e um cliente CLI
com saída JSON. O jogo continua aberto entre comandos. O controle é opcional:
sem `--control`, nenhum socket é aberto. Não exige Node.js no jogo, navegador
ou servidor web. O adaptador Three.js também é opcional.
MCP stdio usa o mesmo executável e API. O SDK para scripts de agentes roda em Node.js.

## Começar

Compile o executável e o exemplo:

```powershell
npm run build:control
cargo build --locked
New-Item -ItemType Directory -Force artifacts | Out-Null
.\target\debug\peregrust.exe dist/control-demo.js --root . --control artifacts/game-session.json
```

Em outro terminal, com o mesmo diretório de trabalho:

```powershell
.\target\debug\peregrust.exe ctl --session artifacts/game-session.json control.describe
.\target\debug\peregrust.exe ctl --session artifacts/game-session.json runtime.info
.\target\debug\peregrust.exe ctl --session artifacts/game-session.json scene.query --params '{"name":"player"}'
.\target\debug\peregrust.exe ctl --session artifacts/game-session.json frame.capture --output artifacts/view.png
```

No PowerShell antigo, prefira `--params-file arquivo.json` para evitar problemas
de escaping ao passar JSON para executáveis nativos. O arquivo deve conter
somente o objeto de parâmetros. `ctl --help` descreve as opções do cliente.
Em Linux/macOS, use `./target/debug/peregrust`.

O arquivo de sessão identifica a instância e contém a credencial de acesso.
Use um caminho novo para cada instância; um arquivo existente nunca é
sobrescrito. O arquivo é removido no encerramento normal. Após uma interrupção
forçada, escolha outro caminho ou remova o arquivo da instância encerrada.
`--hidden` pode ser combinado com `--control`; ainda exige uma sessão gráfica
e GPU/driver compatível.

## Registrar a cena e o estado

Depois de criar a cena, câmera e renderer:

```ts
import { attachThree } from 'peregrust/inspect/three';

const detach = attachThree({ scene, camera, renderer }); // nome padrão: main
const unregister = Peregrust.control.registerState('player', () => ({
  health: player.health,
  position: player.position.toArray(),
}));
```

Os dois retornos removem o registro quando chamados. Nomes duplicados são
rejeitados. Use `name` em `attachThree` e `scene` nos parâmetros CLI para
trabalhar com várias cenas. `Peregrust.control.enabled` indica se há controle
externo habilitado; os registros podem existir mesmo quando ele está desligado.

Providers de estado retornam JSON síncrono de até 1 MiB de texto. Não retornam
objetos Three.js, Promises ou referências cíclicas. O estado semântico do jogo
(vida, inventário, regras) é registrado pelo jogo; o runtime não o deduz da GPU.

Ações semânticas usam um schema e um handler síncrono:

```ts
const removeAction = Peregrust.control.registerAction('player.heal', {
  description: 'Recupera pontos de vida do jogador.',
  inputSchema: {
    type: 'object', properties: { amount: { type: 'integer', minimum: 1, maximum: 100 } },
    required: ['amount'], additionalProperties: false,
  },
}, ({ amount }) => {
  player.health += Number(amount);
  return { health: player.health };
});
```

`action.list` descobre os schemas e `action.call` recebe `name`, `input` e
`observe` opcional. O input é validado antes de chamar o handler. Os schemas
aceitam objetos, arrays, strings, números, inteiros, booleanos, `properties`,
`required`, `additionalProperties` booleano, enums primitivos, `items`, limites
numéricos/de comprimento e metadados `title`, `description`, `default`.
Defaults são descritivos, não são inseridos no input. `$ref`, combinações e
keywords não suportadas são rejeitados no registro. Handlers não retornam
Promises; trabalho que depende de quadros futuros deve ser iniciado pela ação
e observado em chamadas posteriores. Retornos JSON têm limite de 1 MiB.

Outros renderizadores podem fornecer `Peregrust.control.registerScene(name,
adapter)` com `query(params)` e `update(params)` síncronos. `capture(params)` é
opcional e retorna uma Promise com `{width, height, pixels}`, com pixels RGBA8
ordenados da linha superior para a inferior. Veja os tipos do SDK.

## Operações

`control.describe` retorna os schemas JSON dos parâmetros aceitos. O cliente
`ctl` sempre imprime uma resposta JSON em stdout, exceto `--help`. Erros de
operação retornam exit code 1; argumentos CLI inválidos retornam 2.

| Operação | Parâmetros principais | Resultado |
| --- | --- | --- |
| `control.describe` | nenhum | versão do protocolo e operações/schemas |
| `runtime.info` | nenhum | quadro, dimensões, cenas e estados registrados |
| `runtime.pause` / `runtime.resume` | nenhum | estado de pausa e tempo de animação |
| `runtime.step` | `frames`, `dtMs`, `observe` | avanço exato enquanto pausado |
| `runtime.logs` | `after`, `limit`, `level` | logs com cursor de sequência |
| `runtime.metrics` | nenhum | média/p95/última duração dos callbacks |
| `scene.list` | nenhum | nomes das cenas |
| `scene.query` | `scene`, `id`, `name`, `type`, `tag`, `fields`, `offset`, `limit` | objetos e paginação |
| `scene.update` | `scene`, `id`, `position`, `rotation`, `scale`, `visible`, `name` | objeto observado após o quadro |
| `state.list` | nenhum | nomes dos providers |
| `state.get` | `name` | JSON do provider |
| `action.list` | `offset`, `limit` | ações registradas e schemas |
| `action.call` | `name`, `input`, `observe` | retorno da ação e observação |
| `input.dispatch` | `event` | confirmação de envio |
| `input.key` | `code`, `key`, `frames`, `observe` | quadros executados e observação opcional |
| `frame.capture` | `scene`, `width`, `height` | PNG em `result.capture` |

As consultas usam igualdade exata. `tag` busca strings em `object.userData.tags`.
Os campos disponíveis são `id`, `name`, `type`, `parent`, `position`, `rotation`,
`scale`, `visible`, `worldPosition` e `tags`. O limite padrão é 50 objetos e o
máximo é 500; `nextOffset: null` indica o fim. A cena pode mudar entre páginas.
`visible` é a propriedade do objeto, não uma prova de visibilidade na câmera.

Os IDs são UUIDs dos objetos Three.js, estáveis durante a vida desses objetos.
Nomes podem se repetir. Objetos removidos da cena deixam de aceitar alterações;
não reutilize IDs após reiniciar o jogo. Transformações são locais ao pai,
`rotation` usa radianos e a ordem Euler do objeto. Alterações modificam o jogo
em memória e podem ser sobrescritas pela lógica do próximo quadro; não editam
arquivos-fonte. Todo o patch é validado antes de modificar um objeto.

Exemplo de resposta:

```json
{
  "ok": true,
  "sessionId": "identificador-da-instancia",
  "frame": 120,
  "result": {
    "objects": [{"id":"uuid-do-objeto","name":"player","position":[0,0,0]}],
    "total": 1,
    "offset": 0,
    "nextOffset": null
  }
}
```

Falhas retornam `ok: false` e `error: {code, message}`. Entre os códigos estão
`INVALID_ARGUMENT`, `METHOD_NOT_FOUND`, `SCENE_NOT_FOUND`, `STATE_NOT_FOUND`,
`OBJECT_NOT_FOUND`, `TIMEOUT` e `STOPPED`. Erros de operação não encerram o jogo.
`frame` está presente nas respostas produzidas pelo JavaScript; erros anteriores
ao processamento, como autenticação ou timeout de transporte, podem não tê-lo.

## Agir e observar

Grave este objeto em um arquivo e passe-o a `input.key --params-file`:

```json
{
  "code": "KeyW",
  "key": "w",
  "frames": 30,
  "observe": {
    "query": {"name":"player","fields":["id","position"]},
    "state": "player",
    "capture": {"width":640,"height":480}
  }
}
```

```powershell
.\target\debug\peregrust.exe ctl --session artifacts/game-session.json input.key --params-file action.json --output artifacts/after.png
```

O runtime envia `keydown` antes dos callbacks, avança 30 quadros concluídos,
envia `keyup` e devolve a observação daquele quadro. O estado e a consulta são
lidos antes do readback assíncrono da imagem. `result.objects` contém o resultado
da consulta, `result.state` contém o estado e `result.capture` contém a imagem.
Callbacks assíncronos são aguardados. Quadros sem desenho também contam.
O intervalo aceito é 1–600 quadros, com padrão 1. Não há repetição automática
de eventos `keydown` durante a tecla mantida pressionada.

`input.dispatch` aceita `keydown`, `keyup`, `pointerdown`, `pointerup`,
`pointermove`, `wheel` e `click`. Eventos de teclado precisam de `code` e `key`.
As coordenadas de mouse são pixels lógicos. Os eventos chegam aos mesmos
listeners do jogo usados pela entrada nativa, sem mover o cursor do sistema.
Quem envia eventos individuais controla o par pressionar/soltar; use
`input.key` para liberação automática. Gamepad sintético não está nesta versão.

O timeout padrão é 10 segundos, configurável por `--timeout-ms` até 60 segundos.
Pedidos expirados antes do processamento são descartados. Se uma ação já
começou, o timeout não desfaz seus efeitos: uma tecla mantida é liberada quando
o runtime retoma o processamento de quadros. Uma nova leitura permite verificar
o estado antes de repetir uma ação. Operações são serializadas por sessão.

## Captura e tempo

O adaptador renderiza a cena/câmera registrada em um render target temporário e
lê os pixels reais da GPU. O PNG tem no máximo 2048×2048; sem dimensões explícitas,
preserva a proporção do drawing buffer e reduz a resolução se necessário.
`--output` grava o PNG no lado do cliente e substitui o base64 por `path` no JSON.
Sem essa opção, a resposta contém `mimeType`, `width`, `height` e `data` base64.

Por padrão, a captura não inclui decoração da janela, composição de múltiplas
câmeras ou pipelines personalizados. Para incluir o pipeline do jogo, passe
`render: () => engine.renderView()` a `attachThree`: o callback deve renderizar
no target atual sem avançar a simulação nem esperar outro RAF. Há uma
renderização extra, que também executa hooks de renderização do Three.js.
O render target anterior é restaurado mesmo se a leitura falhar. Dimensões
personalizadas não alteram a projeção da câmera.

`frame` identifica o quadro dos callbacks, sem garantir que a apresentação da
janela terminou. `runtime.pause` suspende RAF/onFrame e a contagem de quadros;
consultas e capturas continuam disponíveis sem avançar callbacks.
`runtime.step` exige pausa e executa 1–600 quadros com `dtMs` entre 0,001 e 1000
(padrão 1000/60). Os timestamps de RAF/onFrame avançam por esse intervalo exato.
`input.key` enquanto pausado também avança seus N quadros, a 1000/60 ms por quadro,
e permanece pausado ao terminar. `runtime.resume` exclui o intervalo de pausa
do timestamp de animação. Pedidos e mutações durante a pausa podem compartilhar
o mesmo `frame`: ele identifica passos de animação, não revisões do estado.

`Date`, `performance.now`, timers, I/O, input físico e serviços externos continuam
em tempo real. Esta é pausa de callbacks e relógio de animação controlado, não
replay determinístico de todo o jogo. Bibliotecas que usam relógios próprios e
simulações Rust independentes precisam de ações específicas do jogo.

`runtime.metrics` mantém até 240 amostras de duração real dos callbacks, incluindo
Promises aguardadas. Não mede tempo de GPU nem inclui o readback adicional das
capturas. `runtime.logs` mantém 1024 mensagens de console, até 4096 caracteres
cada, com `sequence`, `frame`, `level` e `message`. Passe `nextCursor` como `after`
na próxima leitura. `oldestSequence` permite detectar registros descartados.
Logs são da sessão viva; falhas fatais continuam disponíveis no stderr do processo.

## Transporte compartilhado

O servidor nativo escuta somente em `127.0.0.1`, numa porta efêmera, com token
aleatório por sessão. O arquivo contém `version`, `sessionId`, `address`, `token`
e `pid`. Cada conexão TCP envia um objeto JSON seguido por newline:

```json
{"token":"credencial","method":"scene.query","params":{"name":"player"},"timeoutMs":10000}
```

A resposta também termina em newline; a conexão é fechada em seguida. Limites:
1 MiB por pedido e 32 MiB de resposta no cliente. Use JSON UTF-8 e evite enviar
buffers ou a árvore completa sem limites. O thread de transporte apenas entrega
pedidos e aguarda respostas: todo acesso à cena ocorre no thread do V8, nos
hooks de quadro. Esse transporte interno é distinto do protocolo MCP.

## MCP stdio

```powershell
.\target\debug\peregrust.exe mcp --session artifacts/game-session.json
```

Configure seu cliente MCP para iniciar esse executável com os argumentos
`mcp`, `--session` e o caminho absoluto da sessão. Clientes que usam o formato
`mcpServers` podem receber esta configuração, ajustando os caminhos:

```json
{"mcpServers":{"peregrust":{
  "command":"C:/jogos/peregrust.exe",
  "args":["mcp","--session","C:/jogos/artifacts/game-session.json"]
}}}
```

O jogo deve estar aberto com `--control`. O servidor implementa inicialização,
ping, descoberta e chamada de ferramentas pelo transporte stdio, negociando
MCP `2025-11-25`, `2025-06-18` ou `2025-03-26`. Outros clientes podem aceitar a
versão oferecida durante a negociação. Stdout contém somente JSON-RPC.
Não exige Node.js nem inicia outra janela do jogo.

As ferramentas usam os nomes da API com pontos trocados por underscores:
`scene_query`, `runtime_step`, `action_call`, etc. Schemas vêm de
`control.describe`. Dados ficam em `structuredContent` e em conteúdo textual;
PNGs ficam em blocos MCP de imagem, sem duplicar o base64 no texto.
Falhas da operação retornam `isError: true`; falhas de protocolo usam erros
JSON-RPC. Operações são serializadas e usam `--timeout-ms` (padrão 10000,
máximo 60000). Cancelar no cliente não desfaz uma ação em andamento; o servidor
limita sua espera pelo timeout. Não há transporte HTTP neste adaptador.

## SDK Node.js/TypeScript

Instale/ligue o pacote local Peregrust no projeto do agente e importe somente
`peregrust/client` nesse processo Node.js:

```ts
import { connect } from 'peregrust/client';
const game = await connect('artifacts/game-session.json');
await game.call('runtime.pause');
const result = await game.call('runtime.step', {
  frames: 30, dtMs: 1000 / 60, observe: { state: 'player' },
});
console.log(result.frame, result.result);
await game.capture({ scene: 'main' }, 'artifacts/view.png');
await game.call('runtime.resume');
```

`call` retorna o envelope com quadro/sessão e lança `ControlError` quando a API
retorna falha; `error.code` e `error.response` preservam os detalhes.
`batch([{method, params}, ...])` executa em ordem e para no primeiro erro;
não é transação nem faz rollback. `capture(params, output)` pode salvar o PNG
no cliente. Cada chamada usa diretamente o transporte local, sem abrir um
processo CLI. O cliente fixa a identidade da sessão; reconecte após reiniciar
o jogo. O SDK é para automação externa, não para o JavaScript embarcado.

## Verificação

```sh
npm test
npm run typecheck
cargo test --locked
cargo build --locked
npm run test:control
```

O teste de controle abre um processo nativo, usa a CLI em processos separados,
altera a cena, verifica a duração de entrada e a liberação após timeout, lê
estado, verifica os pixels e a orientação dos PNGs e confere o encerramento da
sessão. Também valida SDK, pausa/step, ações, logs/métricas e interoperabilidade
com o cliente MCP oficial, incluindo imagens. As imagens ficam em `artifacts/control-before.png` e
`artifacts/control-after.png`. O teste exige GPU/driver e sessão gráfica; no
Linux de CI roda sob Xvfb e Vulkan por software.
