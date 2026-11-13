# 0002: Cloud storage over plain WebDAV; default server for the platform

- **Status:** proposed (spike, 2026-11-13). Not wired into the UI.
- **Decides:** how Kantor opens and saves files on a self-hosted drive, and which server the civil-servant platform should ship by default.
- **Prototype:** `crates/kantor_cloud` (library, unit tests, `examples/dav.rs`, `examples/bench.rs`, `bench/` compose files).

## Context

Kantor is the desktop layer of a proposed national collaboration platform for civil servants. Files have to live on a self-hosted drive that the agency runs, and Kantor has to open them, save them back without overwriting a colleague's work, and keep working on poor links (regional offices on 500 kbit/s to 2 Mbit/s with loss).

The user likes Nextcloud but worries that, being PHP, it is slower than Kantor itself. The plan is that Kantor speaks **plain WebDAV** (RFC 4918) so the server stays swappable. This spike checks whether that works and measures two candidate servers through Kantor's own client.

## What was built

`kantor_cloud` is a small, blocking WebDAV client with no GPUI dependency. It reuses the HTTP stack Kantor already has: **ureq 3 + rustls** (workspace dependency, already used by `kantor_ai` and `kantor_update`). No new HTTP or TLS crate was added; the only dependencies are `ureq`, `roxmltree` and `base64`, all already in the workspace.

- `Client::list` (PROPFIND depth 1) and `stat` (depth 0). Only `DAV:` properties are read: resourcetype, getcontentlength, getetag, getlastmodified, getcontenttype. Vendor properties (`oc:fileid` and so on) are ignored, so the same code works against both servers.
- `get` returns bytes plus ETag. It retries `425 Too Early` with backoff for up to 15 s, because OpenCloud answers 425 while it post-processes an upload (see results). `try_get` makes one attempt.
- `put(path, bytes, Precondition, lock)` where `Precondition` is `IfMatch(etag)` (a normal save), `IfNoneMatch` (Save As or a new file) or `Overwrite` (the user chose to overwrite). Returns the new ETag. `412` maps to `Error::Conflict`, `423` to `Error::Locked`.
- `lock` / `unlock` (exclusive write lock, token taken from the `Lock-Token` header or the body). The token is sent in `If: (<token>)` on PUT.
- `mkcol`, `delete`; Basic auth (password or app password) or Bearer token; path segments are percent-encoded; `max_redirects(0)` so a PUT is never replayed somewhere else. The per-read timeout is 600 s, because a 120 s timeout broke 20 MB saves on 500 kbit/s (see Degraded links).
- 9 unit tests: Nextcloud and OCIS multistatus samples (including a 404 propstat and a default-namespace document), lock token parsing, URL encoding, auth header, status mapping.

`cargo run -p kantor_cloud --example dav -- ls|get|put|open|lock|unlock` is the CLI. `open` downloads a file and opens it with the real engines: `.docx` straight from memory with `docs_engine::docx::read`; `.xlsx` through `sheets_engine::Workbook::open`, which only takes a path, so the bytes go to a temp file first (which an offline copy needs anyway). Verified against both servers, all byte-identical round trips:

| file | result | open time (debug build) |
|---|---|---|
| `ledger-50k.xlsx` (49 KB, 1,000 rows) | sheets `["Anggaran"]` | 60–106 ms |
| `tests/corpus/docx/report.docx` uploaded as `Laporan Q3 (final).docx` (spaces and parentheses in the name) | 59 paragraphs, 1,249 words | 63–107 ms |
| `ledger-2m.xlsx` (2.0 MB, 42,000 rows) | sheets `["Anggaran"]` | 1.6–1.9 s |
| `report-20m.docx` (20.3 MB, 22 photos) | 484 paragraphs, 5,568 words | 51–55 ms |

Both servers serve the user's files at the same URL, `/remote.php/dav/files/<user>/`, so one base URL setting covers both.

## Method

### Servers (pinned versions, each verified to exist on the registry on 2026-11-13)

**Nextcloud 35.0.2**, set up as the admin manual recommends for production (`bench/nextcloud/`):
`nextcloud:35.0.2-fpm` (PHP-FPM) behind `nginx:1.30.6-alpine` (the manual's nginx config, trimmed), `postgres:18.7-alpine`, `redis:8.8.4-alpine` for the distributed cache and transactional file locking, APCu as the local cache (the image enables it; verified `memcache.local=APCu`, `memcache.distributed=Redis`, `memcache.locking=Redis`), OPcache tuned (`interned_strings_buffer=32`, `memory_consumption=256`, JIT), FPM pool `pm=dynamic, max_children=64, start_servers=16` (the image default of 5 children is too small), a `cron` container with background jobs set to cron, and `PHP_MEMORY_LIMIT=512M`. Auth: an app password made with `occ user:auth-tokens:add`.

**OpenCloud 7.2.5** (`bench/opencloud/`): `opencloudeu/opencloud:7.2.5`, the stable-channel image. `opencloudeu/opencloud-rolling:9.1.0` also exists and is what `opencloud-compose` defaults to; the stable channel is the one meant for production. The image label and the GitHub repository (`opencloud-eu/opencloud`) both say **Apache-2.0**. It runs as `opencloud-compose` runs it: a single Go binary with built-in IDM, NATS and decomposed storage on a local volume. Two changes for the benchmark:
- No Traefik/TLS, because Nextcloud is also measured over plain HTTP. The built-in IdP refuses to start without an `https://` issuer, so it is excluded (`OC_EXCLUDE_RUN_SERVICES=idp`). Kantor signs in with app tokens (`PROXY_ENABLE_APP_AUTH=true`, `auth-app` service, `opencloud auth-app create`), which is how a desktop client would sign in in production too.
- `OC_URL` must use the port the container listens on (9200): the frontend PUTs to the data service at `OC_URL` from inside the container, and every PUT failed with HTTP 500 until host and container ports matched.

ownCloud OCIS was not needed, since OpenCloud worked.

### Client and network

- All measurements go through `kantor_cloud` (`examples/bench.rs`, debug build, one keep-alive connection per client), from the host to `127.0.0.1`.
- Docker: no root and the system daemon was off, so a **rootless dockerd 29.9.2** (rootlesskit 3.3.0 with slirp4netns 1.3.7, builtin port driver) ran with all data under `~/.cache`. Host-to-container traffic therefore goes through rootlesskit's userspace port forwarder, which adds latency and CPU for both servers alike.
- **Degraded links:** Toxiproxy 2.13.0 (`ghcr.io/shopify/toxiproxy:2.13.0`) forwards TCP to each server. Toxiproxy alone can't drop packets, so a sidecar container sharing its network namespace (with `NET_ADMIN` inside that namespace; that works rootless) puts **`tc netem`** on that namespace's `eth0`. Every byte, client to server and server to client, leaves Toxiproxy through `eth0` once, so a single egress qdisc shapes both directions:
  - Profile A, "2 Mbps / 100 ms / 1 %": `netem delay 50ms rate 2mbit loss 1% limit 200`
  - Profile B, "500 kbps / 300 ms / 3 %": `netem delay 150ms rate 500kbit loss 3% limit 50`
  - That is the RTT split as half each way, and the rate and loss apply to each direction. The queue limit is about 1.2 s of buffer. A first try with `limit 100000` made a deep bufferbloat queue, and a 20 MB GET to Nextcloud died after 4 minutes; those numbers were thrown away.
- **Files:** generated by a seeded script: `ledger-50k.xlsx` (49,531 B), `ledger-2m.xlsx` (2,033,339 B), both a ledger with dates, names, amounts, shared strings and a formula column; and `report-20m.docx` (20,300,194 B), a field report with 22 noise PNG photos that don't compress. Shown in the tables as 48 KB / 1.9 MB / 19.4 MB (KiB/MiB).
- **Latency:** each operation 20× after one warm-up. A PUT is a real save: `If-Match` the ETag from the previous PUT. Reported in ms: median, p95 (nearest rank), min, max.
- **Load:** 20 client threads for 60 s, each looping PROPFIND of the 500-file folder, GET of the 2 MB file, and PUT `If-Match` of its own 50 KB file.
- **Resources:** `docker stats` snapshots about every 2–3 s, summed over each stack's containers (Nextcloud = app + nginx + postgres + redis + cron). 100 % CPU = one core.

### The machine, and what else was running

AMD Ryzen 5 PRO 4650U (6 cores / 12 threads, laptop), 31 GB RAM, NVMe with btrfs (zstd). The machine was **heavily loaded by other work** for most of the spike: a bioinformatics assembly (`megahit`, `spades`, about 4–6 cores), other agents' Rust builds (a release build of `kantor`, a wasm build, IronCalc test binaries), and this spike's own compiles. The 1-minute load average was **20–65** during most runs (1–9 in the quiet window near the end, used for the "quiet machine" tables); CPU pressure (`/proc/pressure/cpu` "some") was about 46–50 %. Load averages are logged next to each run in the raw data. So:

- Absolute numbers are pessimistic and noisy. Repeated passes differ by up to 4× (for example NC PUT 48 KB median: 1,912 → 422 → 451 ms).
- Comparisons are only meaningful where the gap is consistent across passes, so the servers were measured interleaved (NC, OC, NC, OC…).

One operational incident. The first rootless dockerd ran as a background job with a 2 h limit and was killed mid-run. Its containers kept running as orphans, and the restarted daemon then started a second copy of every container on the same volumes. OpenCloud's IDM then hung opening its boltdb (the orphan held the lock) and every login failed with 401. Nextcloud kept serving. Measurements from that window were discarded, both stacks were wiped and reinstalled, and everything after "fresh install" below comes from the clean stacks. The direct measurements were all taken before the incident.

## Results

### Correctness: two writers, ETags, locks

Alice and Bob both open the file and get the same ETag. Alice saves with `If-Match`; Bob saves with the stale ETag. Then: `If-None-Match: *` on an existing file, and LOCK tests. Both "writers" are the same account with separate clients; WebDAV lock tokens don't depend on the user. Lock behaviour between two different accounts was not tested.

| check | Nextcloud 35.0.2 | OpenCloud 7.2.5 |
|---|---|---|
| Alice `PUT If-Match` current ETag | 204, new ETag (all runs) | 204, new ETag (all runs) |
| Bob `PUT If-Match` stale ETag | **Accepted (lost update) in 1 of 14 runs on clean stacks**: the very first conflict test on the freshly installed stack. 412 in the other 13, including 5 runs on brand-new files. (One more acceptance, plus 6 correct 412s, came during the orphaned-container incident and is discounted.) | 412 in all 14 runs on clean stacks |
| `PUT If-None-Match: *` on an existing file | 412 (correct) | **201/204, file overwritten (all 4 runs)**: not RFC 7232-compliant |
| LOCK, then a PUT without the token | 423 | 423 |
| second LOCK while locked | 423 | 423 |
| PUT with the lock token, UNLOCK | ok, ok | ok, ok |
| ETag on PUT response | yes (`ETag` and `OC-ETag`) | yes |
| GET right after PUT | always 200 | sometimes **425 Too Early** (async post-processing); see below |

Nextcloud's LOCK is real (the `files_lock` app ships enabled in 35), not the Sabre "fake locker" older versions used.

Save, then reopen at once (PUT, then GET retried every 20 ms until it is served; 10 runs each, ms, direct link):

| server | file | median | p95 | 425 replies |
|---|---|---|---|---|
| Nextcloud | 48 KB | 181.8 | 308.0 | 0 |
| Nextcloud | 1.9 MB | 196.3 | 292.3 | 0 |
| Nextcloud | 19.4 MB | 410.3 | 735.8 | 0 |
| OpenCloud | 48 KB | 153.1 | 287.6 | 2 |
| OpenCloud | 1.9 MB | 148.9 | 873.0 | 8 |
| OpenCloud | 19.4 MB | 248.2 | 855.5 | 0 |

Before the 425 retry was added to `get`, the benchmark aborted on a 425 from OpenCloud. A client that doesn't expect 425 will show the user an error right after saving.

### Latency, direct, quiet machine (the cleanest numbers)

At about 17:00 the other workloads finished, and the 1-minute load average fell to 6–9 (from 20–65). Two more interleaved passes ran then, on the clean reinstalled stacks with the final client (425 retry, 600 s read timeout). 20 repetitions, ms:

| setup | operation | median | p95 | min | max |
|---|---|---|---|---|---|
| nc q1 | PROPFIND depth 1, 500 entries | 149.3 | 156.8 | 128.3 | 161.8 |
| nc q1 | GET 48 KB | 101.6 | 110.9 | 87.6 | 112.2 |
| nc q1 | PUT If-Match 48 KB | 249.2 | 267.7 | 219.7 | 307.0 |
| nc q1 | GET 1.9 MB | 136.4 | 149.4 | 116.5 | 157.1 |
| nc q1 | PUT If-Match 1.9 MB | 273.5 | 300.3 | 240.9 | 302.5 |
| nc q1 | GET 19.4 MB | 208.2 | 229.7 | 181.7 | 235.7 |
| nc q1 | PUT If-Match 19.4 MB | 337.3 | 385.6 | 307.8 | 389.1 |
| oc q1 | PROPFIND depth 1, 500 entries | 174.7 | 211.8 | 162.5 | 214.9 |
| oc q1 | GET 48 KB | 56.3 | 68.0 | 51.3 | 69.4 |
| oc q1 | PUT If-Match 48 KB | 87.5 | 181.9 | 71.6 | 187.0 |
| oc q1 | GET 1.9 MB | 64.6 | 73.6 | 57.8 | 76.7 |
| oc q1 | PUT If-Match 1.9 MB | 93.9 | 218.4 | 80.1 | 319.4 |
| oc q1 | GET 19.4 MB | 95.7 | 109.3 | 87.6 | 116.0 |
| oc q1 | PUT If-Match 19.4 MB | 185.5 | 195.6 | 171.6 | 208.8 |
| nc q2 | PROPFIND depth 1, 500 entries | 159.7 | 181.1 | 125.7 | 188.9 |
| nc q2 | GET 48 KB | 104.5 | 112.0 | 93.1 | 113.4 |
| nc q2 | PUT If-Match 48 KB | 259.6 | 279.1 | 232.2 | 286.6 |
| nc q2 | GET 1.9 MB | 102.0 | 116.8 | 91.8 | 131.5 |
| nc q2 | PUT If-Match 1.9 MB | 270.6 | 288.7 | 226.0 | 304.2 |
| nc q2 | GET 19.4 MB | 207.9 | 231.6 | 177.3 | 249.6 |
| nc q2 | PUT If-Match 19.4 MB | 355.1 | 381.9 | 315.4 | 383.6 |
| oc q2 | PROPFIND depth 1, 500 entries | 201.0 | 286.2 | 151.6 | 300.8 |
| oc q2 | GET 48 KB | 53.2 | 61.6 | 44.9 | 63.7 |
| oc q2 | PUT If-Match 48 KB | 165.1 | 190.1 | 73.9 | 191.3 |
| oc q2 | GET 1.9 MB | 63.8 | 73.4 | 55.2 | 142.6 |
| oc q2 | PUT If-Match 1.9 MB | 93.4 | 212.5 | 75.6 | 307.6 |
| oc q2 | GET 19.4 MB | 108.2 | 116.9 | 93.6 | 118.8 |
| oc q2 | PUT If-Match 19.4 MB | 185.4 | 213.2 | 176.5 | 251.2 |

On a quiet machine, **OpenCloud answers single GETs and PUTs about 1.6–2.9× faster** (it saves 45–180 ms per request). **Nextcloud lists the 500-entry folder about 15–25 % faster.** Nextcloud's numbers are also tight (p95 within about 15 % of the median); OpenCloud's 48 KB and 1.9 MB PUTs have a long tail (p95 about 2× the median).

Save, then reopen at once on the quiet machine (10 runs, ms): Nextcloud 112 / 94 / 174 (48 KB / 1.9 MB / 19.4 MB, median), OpenCloud 53 / 56 / 99, with no 425 replies this time.

### Latency, direct, loaded machine (load average 20–65), 20 repetitions, ms

Three interleaved passes per server. OpenCloud pass 2 was redone after the 425 fix. Raw numbers:

| setup | operation | median | p95 | min | max |
|---|---|---|---|---|---|
| nc p1 | PROPFIND depth 1, 500 entries | 367.8 | 514.7 | 189.6 | 661.1 |
| nc p1 | GET 48 KB | 255.0 | 454.7 | 167.8 | 1062.5 |
| nc p1 | PUT If-Match 48 KB | 1912.1 | 3492.7 | 399.1 | 5706.9 |
| nc p1 | GET 1.9 MB | 306.5 | 510.6 | 164.5 | 534.4 |
| nc p1 | PUT If-Match 1.9 MB | 1300.0 | 2977.0 | 687.6 | 10007.3 |
| nc p1 | GET 19.4 MB | 680.2 | 1362.8 | 361.6 | 1521.8 |
| nc p1 | PUT If-Match 19.4 MB | 1299.7 | 5302.3 | 756.0 | 5649.2 |
| nc p2 | PROPFIND depth 1, 500 entries | 311.8 | 677.1 | 186.7 | 733.8 |
| nc p2 | GET 48 KB | 183.7 | 337.8 | 145.7 | 405.9 |
| nc p2 | PUT If-Match 48 KB | 421.6 | 961.5 | 345.4 | 1022.2 |
| nc p2 | GET 1.9 MB | 213.7 | 406.0 | 154.6 | 552.2 |
| nc p2 | PUT If-Match 1.9 MB | 434.5 | 585.1 | 359.6 | 708.9 |
| nc p2 | GET 19.4 MB | 294.2 | 607.0 | 239.0 | 647.3 |
| nc p2 | PUT If-Match 19.4 MB | 842.9 | 1466.9 | 501.9 | 1937.9 |
| nc p3 | PROPFIND depth 1, 500 entries | 211.9 | 228.6 | 177.8 | 244.4 |
| nc p3 | GET 48 KB | 172.0 | 230.9 | 143.4 | 271.0 |
| nc p3 | PUT If-Match 48 KB | 450.5 | 621.6 | 309.9 | 666.1 |
| nc p3 | GET 1.9 MB | 152.4 | 175.3 | 136.2 | 195.1 |
| nc p3 | PUT If-Match 1.9 MB | 470.4 | 721.7 | 358.4 | 870.6 |
| nc p3 | GET 19.4 MB | 256.6 | 490.6 | 175.9 | 509.0 |
| nc p3 | PUT If-Match 19.4 MB | 519.2 | 874.7 | 467.4 | 1043.3 |
| oc p1 | PROPFIND depth 1, 500 entries | 461.5 | 795.0 | 307.8 | 827.9 |
| oc p1 | GET 48 KB | 114.5 | 262.6 | 84.3 | 281.1 |
| oc p1 | PUT If-Match 48 KB | 1684.7 | 3084.7 | 127.3 | 3509.7 |
| oc p1 | GET 1.9 MB | 126.1 | 181.7 | 106.7 | 192.6 |
| oc p1 | PUT If-Match 1.9 MB | 311.7 | 2151.2 | 152.4 | 2276.3 |
| oc p1 | GET 19.4 MB | 284.1 | 425.4 | 227.7 | 461.9 |
| oc p1 | PUT If-Match 19.4 MB | 886.7 | 2889.0 | 506.8 | 3397.9 |
| oc p2 | PROPFIND depth 1, 500 entries | 368.4 | 434.5 | 316.7 | 468.1 |
| oc p2 | GET 48 KB | 119.5 | 286.5 | 92.0 | 288.0 |
| oc p2 | PUT If-Match 48 KB | 188.3 | 585.7 | 122.1 | 727.3 |
| oc p2 | GET 1.9 MB | 78.7 | 101.8 | 62.3 | 104.5 |
| oc p2 | PUT If-Match 1.9 MB | 228.0 | 1048.9 | 107.9 | 1622.0 |
| oc p2 | GET 19.4 MB | 191.9 | 286.6 | 134.7 | 395.0 |
| oc p2 | PUT If-Match 19.4 MB | 420.1 | 497.7 | 247.6 | 591.1 |
| oc p3 | PROPFIND depth 1, 500 entries | 358.3 | 442.0 | 295.4 | 469.1 |
| oc p3 | GET 48 KB | 116.0 | 146.8 | 85.8 | 288.9 |
| oc p3 | PUT If-Match 48 KB | 764.9 | 1952.8 | 296.9 | 2669.3 |
| oc p3 | GET 1.9 MB | 127.5 | 199.2 | 94.7 | 224.6 |
| oc p3 | PUT If-Match 1.9 MB | 172.9 | 219.6 | 131.3 | 400.1 |
| oc p3 | GET 19.4 MB | 228.4 | 358.3 | 176.3 | 378.8 |
| oc p3 | PUT If-Match 19.4 MB | 321.7 | 417.4 | 292.2 | 491.3 |

Summary (median of the three pass medians, ms; lower is better):

| operation | Nextcloud | OpenCloud | consistent across passes? |
|---|---|---|---|
| PROPFIND 500 entries | 311.8 | 368.4 | NC faster in all 3 |
| GET 48 KB | 183.7 | 116.0 | OC faster in all 3 |
| PUT 48 KB | 450.5 | 764.9 | no (OC faster in p1/p2, slower in p3; both bimodal) |
| GET 1.9 MB | 213.7 | 126.1 | OC faster in all 3 |
| PUT 1.9 MB | 470.4 | 228.0 | OC faster in all 3 |
| GET 19.4 MB | 294.2 | 228.4 | OC faster in all 3 |
| PUT 19.4 MB | 842.9 | 420.1 | OC faster in all 3 |

Uploading the 500 files for the listing test (8 parallel clients, PUT `Overwrite`): first install NC 61 s / OC 172 s (load average about 50); fresh reinstall NC 30 s / OC 18 s (load average about 10). The machine load swamps the server difference here.

### Idle resources (no requests, 90 s, both stacks up)

| stack | CPU mean % | CPU max % | RAM mean MiB | RAM max MiB |
|---|---|---|---|---|
| Nextcloud (first install) | 9.3 | 36.7 | 398 | 417 |
| OpenCloud (first install) | 1.4 | 9.2 | 146 | 147 |
| Nextcloud (fresh reinstall) | 22.3 | 75.5 | 512 | 556 |
| OpenCloud (fresh reinstall) | 1.2 | 13.7 | 268 | 331 |
| Nextcloud (quiet machine, after the load tests) | 13.1 | 72.6 | 459 | 499 |
| OpenCloud (quiet machine, after the load tests) | 0.9 | 10.5 | 372 | 382 |

Nextcloud's idle CPU is mostly the cron container (background jobs every 5 min: 4–15 % mean) and PostgreSQL. Nextcloud RAM by container (fresh): app 363 MiB, cron 87, postgres 42, nginx 12, redis 8.

### 20 concurrent clients for 60 s

| run | server | requests/s | errors | PROPFIND 500 median / p95 ms | GET 2 MB median / p95 ms | PUT 50 KB median / p95 ms | CPU mean / max % | RAM mean / max MiB |
|---|---|---|---|---|---|---|---|---|
| 1 (load avg ~45–60) | Nextcloud | 8.3 | 0 | 909 / 1,969 | 843 / 2,533 | 3,440 / 15,337 | 155 / 401 | 464 / 489 |
| 1 (load avg ~45–60) | OpenCloud | 6.3 | 0 | 1,192 / 3,531 | 299 / 1,123 | 7,500 / 12,533 | 191 / 346 | 320 / 380 |
| 2 (fresh, load avg ~12–24) | Nextcloud | **21.9** | 1 (nginx 502) | 660 / 1,202 | 539 / 989 | 1,204 / 2,537 | 401 / 741 | 550 / 585 |
| 2 (fresh, load avg ~12–24) | OpenCloud | **14.2** | 0 | 2,013 / 4,099 | 937 / 1,293 | 997 / 1,475 | 357 / 667 | 328 / 367 |
| 3 (quiet, load avg 9 before, about 20 during) | Nextcloud | **28.7** | 5 (nginx 502) | 474 / 690 | 407 / 617 | 955 / 2,096 | 504 / 757 | 507 / 535 |
| 3 (quiet, load avg 9 before, about 20 during) | OpenCloud | **20.8** | 0 | 1,501 / 1,852 | 624 / 744 | 684 / 828 | 478 / 601 | 419 / 448 |

The load generator (20 threads, debug build) runs on the same 12-thread machine, so it competes with the server for CPU.

Under concurrency Nextcloud served **1.3–1.5× more of this mix** than OpenCloud in all three runs, mostly because OpenCloud's 500-entry PROPFIND slows down under load (1.5–2 s median). OpenCloud had the lower PUT latency in the two cleaner runs. Nextcloud's CPU splits about evenly between PHP-FPM and PostgreSQL (run 3: app 271 %, db 209 %). Nextcloud returned a few `502` (nginx: "upstream prematurely closed connection" on PUT, 6 of about 3,100 requests over runs 2 and 3; the cause was not found, and FPM logged nothing). OpenCloud returned no errors.

### Degraded links

Through Toxiproxy + netem, on the clean reinstalled stacks (load average 1–21 during these runs). ms; n = repetitions.

**Profile A: 2 Mbit/s, 100 ms RTT, 1 % loss**

| operation | n | Nextcloud median | NC p95 | OpenCloud median | OC p95 |
|---|---|---|---|---|---|
| PROPFIND 500 entries | 20 | 1,226.6 | 1,346.6 | 1,135.9 | 1,283.5 |
| GET 48 KB | 20 | 480.1 | 538.4 | 367.0 | 395.8 |
| PUT If-Match 48 KB | 20 | 515.4 | 832.3 | 339.7 | 456.2 |
| GET 1.9 MB | 20 | 8,912.5 | 9,125.7 | 8,820.8 | 9,011.8 |
| PUT If-Match 1.9 MB | 20 | 8,947.9 | 9,180.6 | 8,871.1 | 8,989.9 |
| GET 19.4 MB | 3 | 86,498.4 | 86,656.7 | 86,453.0 | 86,637.8 |
| PUT If-Match 19.4 MB | 3 | 86,593.3 | 86,862.6 | 86,606.8 | 86,798.4 |

**Profile B: 500 kbit/s, 300 ms RTT, 3 % loss**

| operation | n | Nextcloud median | NC p95 | OpenCloud median | OC p95 |
|---|---|---|---|---|---|
| PROPFIND 500 entries | 10 | 4,041.0 | 4,282.5 | 10,038.7 | 12,405.8 |
| PROPFIND 500 entries (later run, 600 s timeout client) | 1 | 4,141.0 | | 4,133.1 | |
| GET 48 KB | 20 | 1,309.7 | 1,496.3 | 1,398.1 | 4,059.2 |
| PUT If-Match 48 KB | 20 | 1,261.3 | 1,380.9 | 2,527.0 | 3,400.0 |
| GET 1.9 MB | 5 | 35,509.0 | 35,989.5 | 35,660.4 | 36,089.2 |
| PUT If-Match 1.9 MB | 5 | 35,811.0 | 36,109.3 | 35,631.9 | 36,502.5 |
| GET 19.4 MB | 1 | 351,135.2 | | 350,191.5 | |
| PUT If-Match 19.4 MB | 1 | 350,964.5 | | 349,835.9 | |

What happened along the way (all measured, all relevant to the UI work):

- With the first client (120 s per-read timeout), **the 20 MB PUT timed out on both servers** on profile B (after 213 s on Nextcloud, 123 s on OpenCloud). ureq starts waiting for the reply once the body sits in local socket buffers, and on 500 kbit/s those buffers (kernel, rootlesskit, Toxiproxy) hold minutes of data. With a 600 s timeout both servers completed in about 350 s. A real client needs a progress-aware timeout, or chunked/resumable uploads.
- Right after that aborted PUT, Nextcloud answered the next GET with **423 Locked**: its transactional file lock stays held while the server still processes the abandoned upload. OpenCloud served the GET, but it took 568.6 s instead of about 350 s, presumably because the aborted upload was still draining over the same link.
- The 10 s OpenCloud PROPFIND (versus 4.1 s on the later run) and its 4 s GET p95 on profile B were not reproduced. They came from the same run as the aborted PUT and may share its cause. Not investigated further.
- PROPFIND replies are big: the 500-entry listing is 207,618 bytes (Nextcloud) and 210,607 bytes (OpenCloud) uncompressed. Asked with `Accept-Encoding: gzip`, **Nextcloud compresses it to 13,620 bytes (15×); OpenCloud doesn't compress**. `kantor_cloud` doesn't ask for gzip yet (ureq's `gzip` feature is off), so the listings above are uncompressed for both servers. With gzip, Nextcloud's listing on profile B should drop from about 4 s to well under 1 s (not measured).
- **Caveat on loss:** netem drops packets on Toxiproxy's own egress, so the sending TCP stack sees the drop locally and retransmits at once. Transfers ran at about 92 % of the shaped rate even with 3 % loss (1.9 MB in 35.5 s ≈ 458 kbit/s), which is better than TCP manages over real 3 % path loss. Treat the profile B transfer times as a best case. The server comparison still holds, since both servers sat behind the same shaping.

On both links the transfer times of the two servers agree to within about 1–3 % for 1.9 MB and 20 MB. Small requests and listings differ by a few hundred ms, which is round trips and server time, and the winner varied from run to run.

## Decision

**Kantor talks plain WebDAV through `kantor_cloud`. Recommended default server for the platform: Nextcloud**, with OpenCloud kept as a supported second server and re-evaluated in about six months. Kantor needs nothing server-specific: the same URL layout, the same `DAV:` properties, ETags on PUT, real LOCKs on both.

Why, against the worry that started this:

1. **The worry is partly right, but it isn't what users will feel.** On a quiet machine, Nextcloud *is* slower per request: 1.6–2.9× for single GETs and PUTs, which is 45–180 ms more per open or save (for example a 1.9 MB save takes 272 ms vs 94 ms). It is faster at listing a 500-file folder (149–160 ms vs 175–201 ms). Under 20 concurrent clients it handled *more* requests per second in all three runs (28.7 vs 20.8 on the quiet run). On the 2 Mbit/s and 500 kbit/s links the link sets the time, and both servers are within a few percent on every transfer (see "Degraded links"). For opening and saving office files, a tuned Nextcloud (FPM pool, Redis, APCu, PostgreSQL) is fast enough. The 100–200 ms only matters on a LAN, where both are fast.
2. **Correctness matters more than 100 ms.** OpenCloud 7.2.5 overwrote an existing file on `PUT If-None-Match: *` in every run, so "Save As / create, but don't clobber" can't be enforced by the server. It also answers `425 Too Early` right after an upload. Both can be worked around in the client (PROPFIND before Save As, which is racy; retry 425), but they are protocol rough edges. Nextcloud's protocol behaviour was correct apart from one unexplained stale `If-Match` acceptance (1 of 14 clean runs, the first test on a fresh install; see above), which needs a follow-up before go-live (see below).
3. **The platform needs more than files.** For a civil-service platform, Nextcloud's mature ecosystem matters: groupware, Talk, admin tooling, LDAP/SAML, existing European public-sector use (Germany's openDesk bundles it; not re-checked for this spike), and many admins who already know it. OpenCloud is younger (forked January 2025), the stable channel is at 7.2 while rolling is at 9.1, and our setup hit operational snags: the IdP refuses plain HTTP, `OC_URL` must match the container port, and app tokens plus all logins stopped working while a stale duplicate container held the embedded IDM's database lock. (That last one was caused by our incident; Nextcloud kept serving through the same incident.)
4. **OpenCloud's advantages are real:** about 2× faster single requests, lower idle CPU (about 1 % vs 9–22 % for the Nextcloud stack, mostly cron and PostgreSQL), less idle RAM (146–372 MiB vs 398–512 MiB), one binary, Apache-2.0 (Nextcloud server is AGPL-3.0, which matches Kantor's own licence anyway), and no 502s under load. An agency with many small sites and little ops staff could reasonably pick it, so Kantor must keep working with it.

Because Kantor only speaks WebDAV, this choice can be revisited without touching the desktop code.

## Consequences: what UI integration needs next

1. **Accounts and auth.** Settings → "Cloud drive": server URL and user name, then an app password (Nextcloud *Devices & sessions*, OpenCloud *App tokens*) kept in the OS keychain, never in `config.toml`. Later: Nextcloud Login Flow v2 / OIDC device flow so users never paste tokens. TLS is required in production; `kantor_cloud` already uses rustls with Mozilla roots.
2. **Open from server.** A file dialog backed by `Client::list` (PROPFIND depth 1; folder listings were 0.2–0.5 s direct and about 1.1–1.2 s on 2 Mbit/s and about 4 s on 500 kbit/s (uncompressed) on the slow links, so cache listings and show them before refreshing). Download on a background thread with progress, write to a local working copy, then open it through the existing engines (`Workbook::open(path)`; Docs can read bytes). Remember `(server, path, etag)` with the document.
3. **Save back.** Save writes locally first (as today), then `PUT If-Match: <etag>` in the background and records the new ETag. Save As / new file uses `IfNoneMatch`, but OpenCloud ignores it, so also `stat` first and treat an existing file as a conflict. Treat 425 after save as "still processing" (already retried in `get`). Use LOCK only while a document is open for editing, with a timeout and refresh (`lock` already sends `Timeout: Second-N`). Show "locked by X" read-only mode on 423; owner information needs a PROPFIND of `lockdiscovery` (not implemented).
4. **Conflict dialog.** On 412: "This file was changed on the server since you opened it." Options: *Save as a copy* (`Laporan (conflict, Dzul, 2026-11-13).xlsx`, `IfNoneMatch`), *Open theirs and lose mine*, *Overwrite theirs* (explicit `Overwrite`, maybe needing a second confirmation). A real merge isn't feasible for xlsx/docx in v1. Because of the unexplained Nextcloud stale-If-Match acceptance, also compare the server ETag (`stat`) before PUT; that costs one round trip (one 300 ms round trip plus server time on profile B).
5. **Offline copy and sync.** Keep downloaded files in a cache folder (`~/.cache/kantor/cloud/<server>/<path>` plus a small index of etag, size, mtime, dirty flag). Opening while offline uses the cache. Saves while offline set *dirty*; on reconnect, upload with `If-Match: <base etag>`, and fall into the conflict dialog on 412. No background whole-drive sync in Kantor: that's the job of the Nextcloud/OpenCloud desktop clients (or a later dedicated sync daemon). Kantor syncs only the files the user opened.
6. **Slow links.** On 500 kbit/s a 2 MB save takes about 36 s and 20 MB takes about 6 minutes, so saves must never block the UI. Show progress, allow cancel, keep the local copy authoritative until the upload is confirmed. For files over about 10 MB use chunked/resumable upload (Nextcloud chunking v2, OpenCloud TUS); `kantor_cloud` sends one plain PUT and has no resume today. Turn on ureq's `gzip` feature so listings are compressed: Nextcloud shrinks a 500-entry listing 15× when asked, though OpenCloud doesn't compress. Replace the fixed 600 s read timeout with one based on upload progress.
7. **Follow-ups before committing.** Re-run these benchmarks on an idle, server-class machine with TLS. Test locks across two real accounts. Chase the Nextcloud stale-If-Match acceptance (1 of 14 clean runs) with server logs (does it reproduce after `occ maintenance:repair` and a cold Redis?) and report it upstream if it reproduces. Report OpenCloud's `If-None-Match` behaviour upstream.

## Update 2026-11-14: built (branch `feat/cloud-storage`)

The UI described above now exists in Kantor (see README → *Cloud storage*). What building it
taught us about the servers:

- **Nextcloud changes a file's ETag on every LOCK and UNLOCK** (`files_lock`, Nextcloud 35), and
  `getlastmodified` stays the same. Comparing ETags alone made every save after taking a lock a
  false conflict. A working copy now records the server's ETag, modification time, size and the
  SHA-256 of its bytes. A new ETag with a different time or size is a change. A new ETag with the
  same time and size is settled by downloading the file and comparing the hash: time-and-size
  alone was tried first, and on OpenCloud it let another client's quick, same-size save through
  (getlastmodified has one-second resolution). When the version is unchanged, the PUT sends the
  current ETag. Opening locks first, then downloads.
- **Nextcloud answers `UNLOCK` with an unknown token with HTTP 500**, not 409/412. Releasing a
  lock forgets it on any answer from the server, and keeps it for the next start only when the
  server can't be reached.
- **A refused PUT that carries a lock token** can mean that the lock is gone (expired while the
  computer slept), not that the file changed. Nextcloud answers 412, OpenCloud 7.2 answers 500.
  When refreshing the lock fails, Kantor retries without the token, and the app takes a new lock.
- After an upload is cut off mid-way, Nextcloud can keep its own file lock for a moment. The
  next save gets 423 even though the lock is ours. Kantor treats that as "busy, retry".

End-to-end tests (`crates/kantor_cloud/tests/e2e_server.rs`, run by `bench/e2e.sh` against the
Nextcloud stack, with two accounts, alice and bob) cover open, edit and save; a conflict from a
second client; a lock held by another account; offline, then Sync now; an upload cut off
mid-way; a lost lock; and crash cleanup. All 7 passed against Nextcloud 35.0.2. Against
OpenCloud 7.2.5, with one account, so the second-account lock test was skipped, the other 6
passed.

## Raw data and reproduction

- Compose files: `crates/kantor_cloud/bench/{nextcloud,opencloud,link}/`. Toxiproxy proxies: 28080 → Nextcloud `web:80`, 29200 → OpenCloud `opencloud:9200`; netem is applied with `docker exec kspike-link-netem-1 tc qdisc replace dev eth0 root netem …`.
- Benchmarks: `cargo run -p kantor_cloud --example bench -- setup bench/500 500`, `latency <label> 20 bench/500 <files…>` (a `path:reps` suffix sets per-file repetitions), `conflict [path]`, `ready <label> 10 <files…>`, `load 20 60 bench/500 <2 MB file> <50 KB file>`, with `KANTOR_DAV_URL`, `KANTOR_DAV_USER`, `KANTOR_DAV_PASS`.
- All containers and volumes were removed after the spike.
