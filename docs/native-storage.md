# Armazenamento para extensões Rust

Uma extensão incorporada ao executável pode obter um handle com
`state.borrow::<peregrust::storage::StorageBackend>().native_handle()`.
`NativeStorage` é `Send + Sync`, clonável e independente do V8. A clonagem
copia somente os caminhos, sem duplicar o cache de localStorage.

Execute suas operações síncronas em um worker de I/O, por exemplo com
`tokio::task::spawn_blocking`. Não retenha empréstimos de `OpState` durante
um `await`. Limite operações pendentes na extensão para evitar filas de
payloads grandes.

`binary_get` transfere um `Vec<u8>` ao chamador. `binary_set` recebe uma fatia
emprestada e grava cabeçalho e payload separadamente, sem construir outra
cópia do payload. O proprietário deve manter os bytes vivos até a conclusão
da gravação. `binary_set_validated` valida o valor novo e o existente sob o
mesmo lock do namespace antes de publicar a substituição.

As gravações usam arquivo temporário, sincronização e substituição atômica.
Backups, tombstones e importação somente de leitura dos saves Mystral mantêm
o contrato do armazenamento JavaScript. `binary_has` e `Peregrust.storage.has`
consultam a existência sem carregar o payload. A validação do conteúdo do
save continua sendo responsabilidade da extensão; o runtime valida o seu
próprio envelope de armazenamento.

No host Wuxia, checkpoints percorrem simulação Rust → armazenamento Rust e
o caminho inverso por transferência de `Vec`, sem passar o payload por V8.
Consultas entregam buffers externos ao V8 com propriedade exclusiva até a
coleta de lixo. Comandos e projeções mantêm seus buffers compartilhados e
seu protocolo de sincronização.

“Sem cópia” descreve essas transferências de payload entre subsistemas.
Serialização, reconstrução do estado, leitura/gravação pelo sistema
operacional e criação de objetos JavaScript continuam tendo custo.
