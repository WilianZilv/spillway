# spillway

Sistema de arquivos **RAM-first** para gravações de vídeo, com balanceamento de carga entre discos e cluster **master / nodes**.

Faz num único binário Rust o que normalmente exige montar um tmpfs, juntar tudo com mergerfs e manter um script movendo arquivos para os discos:

```
               gravadores / players
                        │  /videos  (FUSE)
              ┌─────────▼──────────┐
              │   spillway master  │  painel web :7070
              │  RAM (tmpfs spool) │──────────────────────┐
              │  parse MP4 · thief │                      │ HTTP (token)
              │  balanceador       │                      │
              └──┬──────────────┬──┘              ┌───────▼────────┐
                 │ node local   │                 │ spillway node  │  outro servidor
          /mnt/disk1      /mnt/disk2              │   /mnt/disk3   │
                                                  └────────────────┘
```

## Como funciona

1. **Escrita**: todo arquivo criado em `/videos` vai para a RAM (spool em tmpfs montado pelo próprio spillway).
2. **Pronto para mover**: o spillway *é* o sistema de arquivos, então sabe exatamente quem está com o arquivo aberto. Um arquivo está pronto quando **não tem nenhum handle de escrita aberto** e ficou **`settle_secs` sem escrita** depois do último close. Isso cobre quem fecha e reabre para anexar.
3. **Parse**: ao selar, o arquivo é analisado sem decode: estrutura MP4/fMP4 (`mvhd`, ou soma de `trun` nos `moof` para fMP4 fragmentado), duração, codec e resolução. O spillway é genérico: não interpreta o caminho. Regras da aplicação entram pelos **hooks** (abaixo).
4. **Balanceamento**: cada arquivo vai para um destino escolhido por *smooth weighted round-robin* com peso = espaço livre × multiplicador. No modo normal a proporção é estrita; sob pressão de RAM (`pressure_pct`) usa toda vaga livre. O espaço livre é corrigido pelas cópias em andamento e pelas concluídas depois do último teste, então uma rajada não estoura o `max_use_pct`.
5. **Cópia**: vai para `<destino>/.spillway/tmp`, aplica mtime/modo/dono, depois faz `rename` atômico; só então o arquivo sai da RAM. Localmente usa `copy_file_range`; para nodes remotos, PUT HTTP em streaming. O inode é o mesmo, então quem está lendo não percebe a troca.
6. **Leitura**: RAM primeiro, depois os destinos na ordem configurada (como o `ff` do mergerfs). Destinos remotos são lidos via HTTP com read-ahead de 1 MiB. Há caches de lookups positivos, negativos e de listagens. Se algo fora do spillway apagar ou mover um arquivo nos discos, o `open` confirma a localização (cache com mais de 2 s) e uma leitura que não acha o arquivo procura nos outros destinos.
7. **Modificação depois de movido**: abrir para escrita um arquivo que já está no disco faz *copy-up* de volta para a RAM; ao ser movido de novo, a cópia antiga é apagada. Reabrir durante uma cópia invalida a transferência (versionamento).
8. **Thief**: quando a RAM passa de `thief_max_gb`, descarta os arquivos fechados mais antigos que ainda não foram copiados, até `thief_resume_pct`. Nunca descarta um arquivo aberto. Acima de `hard_limit_pct`, novas escritas recebem `ENOSPC` em vez de esgotar a memória da máquina.

## Hooks

O spillway não sabe o que os arquivos significam. A sua aplicação pluga as regras dela por dois hooks HTTP:

```toml
[hooks.validate]              # antes de mover: move / drop / hold
url = "http://127.0.0.1:7090/spillway/validate"
token = "segredo"             # Authorization: Bearer
batch_max = 256               # o lote sai ao juntar 256 arquivos...
batch_wait_ms = 50            # ...ou quando o mais antigo esperou 50 ms
inflight = 4                  # lotes em voo ao mesmo tempo
timeout_ms = 2000
on_error = "move"             # erro/timeout/sem resposta: "move" (fail-open) ou "hold"
hold_retry_ms = 5000

[hooks.moved]                 # depois de mover: notificação durável
url = "http://127.0.0.1:7090/spillway/moved"
token = "segredo"
batch_max = 500
batch_wait_ms = 1000
timeout_ms = 5000
outbox_dir = "/var/lib/spillway/outbox"   # fora do tmpfs; só no arquivo
```

**validate** recebe um lote de arquivos selados (só metadados):

```json
{ "batch": "v-000172", "files": [
  { "id": "88231:3", "path": "cam-01/2026-10-22/13-37-54.mp4",
    "size": 2034648, "mtime_ms": 1792600000000,
    "mp4": { "ok": true, "duration_sec": 60.2, "fragmented": true, "fragments": 60, "tracks": [ ... ] } } ] }
```

e responde uma decisão por `id` (`fid:versão`; um arquivo modificado ganha outro id e a decisão antiga é ignorada):

```json
{ "decisions": [
  { "id": "88231:3", "action": "move", "labels": { "camera": "cam-01", "site": "loja-centro" } },
  { "id": "88232:1", "action": "drop", "reason": "sem detecção" },
  { "id": "88233:1", "action": "hold", "retry_after_ms": 5000 } ] }
```

* `labels` são livres: o painel só agrupa estatísticas por eles (aba Gravações).
* Arquivos esperando decisão ficam no estado **Validando**; o thief ainda pode descartá-los se a RAM chegar ao teto.
* Arquivo sem resposta, erro HTTP ou timeout seguem `on_error`.

**moved** recebe, em lotes, cada arquivo que chegou a um destino:

```json
{ "batch": "m-004410", "files": [
  { "id": "88231:3", "path": "...", "dest": "disk2", "node": "local", "size": 2034648,
    "mtime_ms": 1792600000000, "moved_at_ms": 1792600065000, "mp4": { ... }, "labels": { ... } } ] }
```

Cada evento é gravado num outbox em disco (`seg-N.jsonl`) **antes** do envio e só sai de lá após um 2xx (a escrita roda fora do runtime async: disco lento não trava o master); falhou, nova tentativa com espera crescente até 30 s. Sobrevive a `kill -9`. Entrega **pelo menos uma vez**: o receptor precisa ser idempotente.

`tests/hookserver.py` é um exemplo completo de receptor dos dois hooks.

### Saúde dos destinos (doctor)

Cada node testa seus destinos a cada 2 s: diretório, `statvfs`, ponto de montagem (`expect_mount`), escrita e releitura de um arquivo de teste. Estados: **Gravando**, **Cheio**, **Suspenso** (N falhas de cópia seguidas, volta no próximo teste bom), **Com falha**, **Offline** (node sem heartbeat), **Só leitura**, **Desativado**.

* **Circuit breaker**: um timeout de I/O tira o destino das buscas por 5 s, então um NFS pendurado nunca trava o mount.
* **`expect_mount`**: o node confere o `st_dev` em **toda** escrita. Se o NFS cair (`nofail`), nada é gravado no disco raiz.
* Toda operação de disco roda num pool limitado por destino e com timeout.
* Uma cópia que falha volta para a fila com backoff e evita aquele destino por `avoid_secs`.

### Reinício e quedas

* `SIGTERM`: desmonta em modo lazy (quem está gravando termina) e continua esvaziando a RAM por `shutdown_flush_secs`.
* `kill -9` ou crash: o spool é um tmpfs e sobrevive ao processo. No próximo start os arquivos são recuperados e movidos, e um mount FUSE órfão é desmontado.
* Node reiniciado: temporários parciais (`.spillway/tmp`) são limpos no primeiro teste.
* **Gravações feitas com o spillway fora do ar**: sem o mount, `/videos` é uma pasta comum do disco local e um gravador (MediaMTX, por exemplo) continua escrevendo ali. Antes de montar, o spillway abre essa pasta; o descritor continua enxergando o que fica embaixo do mount. No start e a cada 10 s, arquivos parados há 10 s são puxados para a RAM, seguem o fluxo normal (parse, hooks, balanceamento) e saem do disco local.
* Um erro interno numa operação do mount responde EIO só àquela chamada e gera um evento no painel; o mount não cai.
* Cada atribuição de destino tem uma geração. O node a devolve nos testes, e o master só envia arquivos depois de um teste da geração atual. Assim, reativar um destino nunca usa o relatório antigo de um node que já o soltou.

## Instalação

Binários prontos para Linux (x86_64 e arm64) saem em cada [release](https://github.com/WilianZilv/spillway/releases):

```bash
curl -fsSL https://github.com/WilianZilv/spillway/releases/latest/download/install.sh | sudo bash
# opções: --version v0.2.0 · --role node · --enable
```

O script baixa o pacote da arquitetura, confere o SHA-256, instala `/usr/local/bin/spillway` e a unit systemd, e cria `/etc/spillway/spillway.toml` só se ainda não existir. Para integrar em outro software, os links fixos `releases/latest/download/spillway-linux-x86_64.tar.gz` (ou `-arm64`) e `SHA256SUMS` sempre apontam para a última versão. Requer `fuse3` no sistema.

Publicar uma versão: suba a versão em `Cargo.toml`, faça o commit e envie a tag (`git tag v0.2.0 && git push origin v0.2.0`). O GitHub Actions testa, compila e publica.

### A partir do código

```bash
# painel (uma vez por mudança no web/)
cd web && pnpm install && pnpm build && cd ..
# binário (Linux) — o painel é embutido
cargo build --release
sudo install -m 755 target/release/spillway /usr/local/bin/spillway
sudo mkdir -p /etc/spillway && sudo cp deploy/spillway.example.toml /etc/spillway/spillway.toml
sudo cp deploy/spillway-master.service /etc/systemd/system/
sudo systemctl enable --now spillway-master
```

Precisa de `fuse3` (`fusermount3`) e de rodar como root (monta tmpfs e FUSE com `allow_other`).

### Configuração no deploy

Tudo o que o painel altera fica no TOML do master: entregue o arquivo pronto e o serviço sobe configurado. [`deploy/spillway.example.toml`](deploy/spillway.example.toml) documenta todas as opções com os padrões.

```bash
spillway check -c /etc/spillway/spillway.toml          # valida; aponta chaves desconhecidas (erro de digitação)
spillway check -c /etc/spillway/spillway.toml --print  # mostra a configuração efetiva, segredos ocultos
```

* **Arquivo gerenciado** (Ansible, repositório de configs): `[web] edit_config = false` deixa o painel só mostrando a configuração; mudanças vêm do arquivo + reinício. Testar destino e esquecer node continuam funcionando.
* **Segredos fora do arquivo**: `SPILLWAY_WEB_TOKEN`, `SPILLWAY_CLUSTER_TOKEN`, `SPILLWAY_VALIDATE_TOKEN` e `SPILLWAY_MOVED_TOKEN` substituem os tokens do TOML. O painel mostra esses campos como definidos pelo ambiente, não deixa editá-los e nunca os grava no arquivo. Os tokens dos hooks não são enviados ao navegador.
* Com `edit_config = true`, salvar pelo painel reescreve o arquivo (os comentários se perdem).

### Conectar um node (outro servidor)

```bash
sudo cp deploy/node.env.example /etc/spillway/node.env   # edite id, master, token, advertise
sudo cp deploy/spillway-node.service /etc/systemd/system/
sudo systemctl enable --now spillway-node
```

O node aparece no painel (**Cluster**). Em **Destinos → Adicionar destino**, escolha o node e o caminho naquele servidor.

## Painel

`http://<master>:7070` (React + shadcn/ui, embutido no binário, ao vivo via SSE):

* **Visão geral**: RAM × teto, entrada/saída, fila, thief, ciclo de vida (escrevendo → selando → fila → em trânsito → discos), vazão, RAM no tempo, saída por destino, eventos.
* **Destinos**: estado, uso, fatia do balanceamento, cópias simultâneas, falhas, teste; adicionar, editar, desativar, só leitura, reordenar, remover.
* **Cluster**: nodes, estado e comando pronto para conectar um novo.
* **Gravações**: parse (válidos, fMP4, duração, bitrate, codecs, resoluções, falhas) e grupos por label do hook validate.
* **Hooks** (na visão geral): latência, lotes, decisões move/drop/hold, falhas e fila do outbox.
* **Arquivos**: navegação no mount mostrando onde cada coisa está (RAM e quais destinos) e últimos arquivos processados.
* **Configurações**: teto de RAM (o tmpfs é redimensionado na hora) e todos os parâmetros, gravados no TOML.

Desenvolvimento do painel: `cd web && SPILLWAY_MASTER=http://localhost:7070 pnpm dev`.

## Testes

```bash
cargo test --release                 # unitários
sudo python3 tests/e2e.py            # 25 cenários end-to-end com mount real
```

Os cenários E2E rodam como root no Linux (precisam de `fuse3`, `python3` e `ffmpeg`) e usam três filesystems ext4 em loop como destinos e um node remoto real. Usam o binário de `target/release`; `SPILLWAY_BIN` aponta para outro.

| cenário | o que prova |
|---|---|
| basic | escrita → parse → cópia; leitura idêntica (md5) antes e depois de mover |
| balance | divisão proporcional ao espaço livre |
| modify | append com reabertura, copy-up após mover, truncate, rename, unlink |
| rules | extensão proibida (EACCES), apagar arquivo movido (EROFS), arquivo vazio descartado |
| readonly | destino vira read-only no meio do tráfego → failover → recupera; nada perdido |
| unmounted | mount some (estilo NFS `nofail`) → nada é gravado no disco raiz |
| full | destino enchendo para antes do limite, zero ENOSPC |
| thief | todos os destinos fora → thief descarta os mais antigos, preserva os novos |
| hardlimit | ENOSPC no limite rígido, memória do host protegida |
| remote | node remoto: leitura via HTTP, `SIGSTOP` (travado) e `SIGKILL`, sem erro para quem grava |
| reenable | destino remoto desativado e reativado em vários pontos do heartbeat: nada é enviado antes do node confirmar, zero falhas |
| burstmoved | 3000 arquivos em 8 s com node remoto e hook moved: nenhum destino cai, nenhuma escrita espera mais que 1,5 s |
| fspanic | erro interno (panic) numa operação do mount vira EIO só naquela chamada; o mount segue no ar |
| underlay | mount fora, o gravador escreve na pasta local embaixo dele: ao voltar, os arquivos são resgatados e movidos |
| deployconfig | `check` acha chave errada; `edit_config = false` bloqueia toda mudança pelo painel; tokens do ambiente valem, não vão ao navegador nem ao arquivo |
| extchanges | arquivo apagado ou movido direto no disco (local e remoto): some do mount em até 2,5 s; leitura segue o arquivo, mesmo com ele aberto |
| crash | `kill -9` no master com arquivos na RAM → recuperados e movidos |
| stop | `SIGTERM` esvazia a RAM antes de sair |
| hookvalidate | hook recusa e rotula arquivos, lotes sob rajada, `moved` recebe exatamente os movidos |
| hookdown | hook fora do ar com fail-open: tudo é movido, falha visível |
| hookslow | hook mais lento que o timeout: não trava a fila |
| hookhold | `hold`: arquivo espera e é decidido de novo |
| hookclosed | `on_error = hold` com hook fora: nada sai da RAM até ele voltar |
| outbox | receptor fora + `kill -9` no master: todos os eventos entregues depois |
| throughput | 5000 arquivos de 200 câmeras: ~150 mil arquivos/min no lab de testes |

## Estrutura

```
src/
  fs.rs        FUSE: união RAM + destinos, copy-up, read-ahead, writeback cache
  ram.rs       spool e ciclo de vida dos arquivos (fila oldest/largest, thief)
  master.rs    selagem + parse, despachante, transferências, thief, tmpfs, shutdown
  cluster.rs   nodes/destinos, estados, circuit breaker, transporte local/HTTP
  balancer.rs  smooth weighted round-robin
  node.rs      doctor, operações de disco protegidas, API HTTP do node
  mp4.rs       parser MP4/fMP4 (sem decode)
  hooks.rs     hook validate (micro-lotes) e hook moved (outbox durável)
  underlay.rs  resgate do que foi gravado na pasta local embaixo do mount
  web.rs       API, SSE, painel embutido, heartbeat do cluster
web/           painel (Vite + React + TypeScript + shadcn/ui)
tests/         e2e.py, camsim.py, lab scripts
deploy/        systemd units e configs de exemplo
```
