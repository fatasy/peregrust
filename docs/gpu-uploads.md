# Uploads WebGPU em lotes

O Peregrust oferece `GPUQueue.writeBufferBatch` no mesmo device/queue usados pelo WebGPU padrão. A extensão é síncrona e aceita um array plano de grupos de cinco valores:

```js
device.queue.writeBufferBatch([
  bufferA, 0, valuesA, 0, valuesA.length,
  bufferB, 16, valuesB, 2, 4,
]);
```

A ordem é `buffer, bufferOffset, data, dataOffset, size`. `bufferOffset` é medido em bytes. `dataOffset` e `size` usam elementos para TypedArrays e bytes para ArrayBuffer/DataView; `undefined` usa offset zero e o restante da view. Os offsets/tamanhos da API explícita precisam ser números inteiros não negativos representáveis com precisão em JavaScript. Fontes compartilhadas não são suportadas.

Cada operação captura seu conteúdo antes do retorno. Erros de argumento interrompem o lote na operação inválida, preservando as anteriores. Erros de validação GPU usam os escopos/eventos do device e permitem continuar nas operações seguintes. A extensão valida faixas antes de acessar memória nativa.

## Agrupamento automático opcional

```js
device.queue.setWriteBufferBatching(true);
device.queue.writeBuffer(bufferA, 0, sharedScratch);
sharedScratch.fill(0); // não muda os bytes do upload anterior
device.queue.writeBuffer(bufferB, 0, sharedScratch);
device.queue.submit([commands]); // libera os uploads antes da submissão
```

O padrão é desligado. `setWriteBufferBatching(false)` libera escritas pendentes antes de voltar ao caminho direto; `flushWriteBufferBatch()` permite liberar explicitamente. O runtime usa uma arena reutilizável para snapshots, começando em 256 KiB e limitada a 4 MiB; escritas maiores seguem diretamente. Os uploads pendentes são enviados antes de esgotar a capacidade da arena ou ao atingir 4096 operações. A memória permanece disponível para reutilização ao desligar o agrupamento e é liberada com o estado do device.

Uploads são liberados antes de `submit`, `writeTexture`, `copyExternalImageToTexture`, `onSubmittedWorkDone`, mudanças de escopo de erros, mapeamento/desmapeamento e destruição de buffers/device. Isso mantém a ordem da fila e a validação no escopo correto. Não combine esse mecanismo com outro monkeypatch que também adie `writeBuffer`.

O agrupamento reduz travessias JS/Rust. O wgpu-core continua criando staging e copiando por destino, e o agrupamento automático acrescenta a cópia do snapshot na arena. Portanto, não é zero-copy nem uma garantia de ganho para todas as cargas. Deve permanecer opcional até ser validado na aplicação.

## Medição

`Peregrust.gpu.setUploadStatisticsEnabled(true)` habilita e zera contadores cumulativos, após liberar uploads pendentes. `getUploadStatistics()` retorna chamadas lógicas, bytes de payload, chamadas nativas diretas/de lote, número de uploads agrupados e submissões. Com controle ativo, o mesmo snapshot fica em `state.get` com `name: "gpuUploads"`.

`nativeCalls = nativeWriteCalls + nativeBatchCalls` conta somente travessias de upload; não todas as operações WebGPU. Bytes são payload solicitado, não VRAM ou bytes efetivamente processados pela GPU. Os contadores são destinados a cargas válidas: um lote explícito que lança erro de argumento pode ter aplicado um prefixo antes do erro, sem concluir a atualização das estatísticas do lote. As estatísticas ficam desligadas por padrão.

O teste `tests/integration/fixtures/gpu-upload-batch.js` verifica snapshots e offsets com readback real, modo direto/agrupado, entrada malformada, buffers de outro device, alinhamento, mapeamento, destruição e continuação após erros de validação. A suíte JS verifica também os pontos de liberação e a arena.
