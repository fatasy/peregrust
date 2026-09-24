# Controle do runtime por IA

O Peregrust oferece uma sessão persistente de controle local e um cliente CLI
com saída JSON. O jogo continua aberto entre comandos. O controle é opcional:
sem `--control`, nenhum socket é aberto. Não exige Node.js no jogo, navegador
ou servidor web. O adaptador Three.js também é opcional.

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
| `scene.list` | nenhum | nomes das cenas |
| `scene.query` | `scene`, `id`, `name`, `type`, `tag`, `fields`, `offset`, `limit` | objetos e paginação |
| `scene.update` | `scene`, `id`, `position`, `rotation`, `scale`, `visible`, `name` | objeto observado após o quadro |
| `state.list` | nenhum | nomes dos providers |
| `state.get` | `name` | JSON do provider |
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

É uma captura da cena registrada: não inclui decoração da janela, composição
de múltiplas câmeras ou pipelines personalizados de pós-processamento. Há uma
renderização extra, que também executa hooks de renderização do Three.js.
O render target anterior é restaurado mesmo se a leitura falhar. Dimensões
personalizadas não alteram a projeção da câmera.

`frame` identifica o quadro dos callbacks, sem garantir que a apresentação da
janela terminou. O relógio continua sendo de tempo real; consultar o runtime
pode solicitar um quadro mesmo em um jogo sem loop contínuo. Esta versão não
oferece pausa, relógio virtual, replay determinístico ou isolamento de timers
e entrada física durante uma observação.

## Transporte e extensão futura

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
hooks de quadro. Esta é uma API própria de controle, não o protocolo MCP.

Um adaptador MCP futuro poderá traduzir ferramentas para estas mesmas operações
e converter `result.capture` em conteúdo de imagem, sem duplicar a lógica de
cena. Logs, métricas detalhadas, ações semânticas registradas, gamepad sintético
e relógio controlado ficam como extensões posteriores.

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
sessão. As imagens ficam em `artifacts/control-before.png` e
`artifacts/control-after.png`. O teste exige GPU/driver e sessão gráfica; no
Linux de CI roda sob Xvfb e Vulkan por software.
