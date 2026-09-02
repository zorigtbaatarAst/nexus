# Nexus

*[English](README.en.md) · Монгол*

**Кодыг байнга ойлгож санадаг платформ.** Nexus төслийг нэг удаа уншаад, түүний бүтэц, түүх,
өмнө нь юу эвдэрч байсныг санана. Тэрнээс хойш бүхэлд нь дахин уншихгүй — юу өөрчлөгдсөнийг
илрүүлж, тэр нь юуг хөндөж байгааг тооцоод, зөвхөн тэр хэсэгт нь шинжилгээ хийнэ.

> **Nexus төслийг ойлгоно; capability-үүд тэр ойлголтыг ашиглана.**

**BugHunter** бол эхний capability — детерминистик алдаа хайлт. Дангаараа ч, Nexus дотроос ч
ажиллана. Дараа нэмэгдэх бүх capability яг ийм хэлбэртэй: индексийг уншаад олдвороо буцаана,
харин таних тэмдэг, амьдралын мөчлөг, түүхийг платформоос үнэгүй авна.

> **Төлөв: ажиллаж байна, гэхдээ дуусаагүй.** Скан, өөрчлөлт илрүүлэлт, frontend-backend
> дамжсан нөлөөлөл, дөрвөн детерминистик дүрэм бүрэн мөчлөгтэйгээ, санах ой, MCP сервер
> бэлэн. Гэхдээ одоогоор ямар ч олдворыг тест ажиллуулж баталгаажуулдаггүй — үүнийгээ бүх
> гаралт дээрээ шууд хэлдэг. [`docs/roadmap.md`](docs/roadmap.md).

## Суулгах

```bash
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh
```

Ганц файл татаж, checksum-ыг шалгаад суулгана. Өөр юу ч хэрэггүй — Java, Node, Docker
шаардахгүй.

Дараа нь ажиллуулна:

```bash
cd /таны/төслийн/зам
nexus scan           # индекслээд baseline тавина
nexus rescan         # юу өөрчлөгдсөн, тэр нь юунд нөлөөлөх вэ
nexus analyze        # BugHunter-ийг ажиллуулна
nexus ask next       # юуг эхэлж харах нь зүйтэй вэ
```

Хоёр binary суулгагдана: `nexus` — платформ, `bughunter` — capability-ийн өөрийнх нь CLI.
Хоёул нэг файл, зөвхөн нэрээрээ ялгаатай (`argv[0]`-оор шийднэ).

`init` гэж тусад нь ажиллуулах шаардлагагүй — `scan` шаардлагатай бол өөрөө бэлдэнэ.

### Шинэчлэх

```bash
# binary — оюун ухаан нь
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh

# Claude Code plugin — prompt-ууд нь
/plugin marketplace update nexus
```

Хоёулаа тусдаа: нэгийг нь шинэчлэхэд нөгөө нь шинэчлэгдэхгүй. Хэрэв өгөгдлийн сангийн схем
binary-аасаа хуучин байвал `nexus doctor` хэлж өгнө — `nexus rescan` ажиллуулахад засагдана.

### Claude Code plugin болгож ашиглах

```
/plugin marketplace add zorigtbaatarAst/nexus
/plugin install nexus@nexus
```

Ингэснээр MCP сервер, найман slash команд, мөн агентад **хэзээ** Nexus рүү хандахыг —
бас хариуг нь **хэрхэн уншихыг** — заасан skill суулгагдана. Зөвхөн MCP серверийг суулгах бол:
`claude mcp add --scope user nexus -- nexus mcp`.

<details>
<summary>Бусад аргууд</summary>

```bash
# эх кодоос нь өөрөө build хийх (Rust хэрэгтэй)
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/nexus/main/install.sh | sh -s -- --from-source

# тодорхой хувилбар, эсвэл өөр хавтас
... | sh -s -- --version v0.1.0 --dir ~/bin

# устгах
... | sh -s -- --uninstall

# repo-г clone хийсэн бол
make install
```
</details>

Ямар нэг зүйл болохгүй бол `nexus doctor` ажиллуулаарай. Юу дутуу байгааг, яаж засахыг тухайн
командтай нь хамт хэлж өгнө.

---

## Гол санаа

**Нотолгоо, түүх, баталгаажуулалтыг Nexus хариуцна. Дүгнэлтийг AI агент хариуцна.**

AI агентад таны бүх repo хэрэггүй. Түүнд хэрэгтэй нь ердөө дөрвөн зүйл: эх кодын зөв хэсэг,
юу өөрчлөгдсөн, тэр өөрчлөлт юуг хөндөж байгаа, мөн тэр газар өмнө нь юу эвдэрч байсан.
Nexus-ийн бүх ажил бол яг үүнийг найдвартай бэлдэж өгөх.

Ингэснээр MCP дэмждэг ямар ч агент ашиглаж чадна — Claude Code, Codex, Copilot, эсвэл
хараахан гараагүй ямар нэг зүйл. Мөн AI-г бүр мөсөн унтраасан ч ажиллана: `scan`, `rescan`,
`impact`, `analyze` бүгд ямар ч model, API key шаардахгүй.

### Аль ч LLM-ээс хамаарахгүй

Агент зөвхөн олдвор **уншиж** чаддаг байсан нь асуудал байсан — олдвор үүсгэж чаддаг цорын
ганц зүйл нь Nexus дотор компайл хийгдсэн код байлаа. HTTP client байхгүй нь биш, яг энэ
тэгш бус байдал л системийг нэг model-оос хамааралтай болгож байсан юм.

`nexus_record_finding` үүнийг арилгана: **ямар ч model одоо provider болно**, Nexus дотор
provider-т зориулсан нэг ч мөр код байхгүй. Агентын бичсэн олдвор нь дүрмийн олдвортой яг
адилхан таних тэмдэг, түүх авна — тиймээс дараагийн сешн дээр давхардахгүй, харин "энэ
өмнө нь бүртгэгдсэн байна" гэж танигдана.

Мэдээж адилхан шалгуур тавина: индекс дотор байхгүй файл заасан "нотолгоо" бол нотолгоо биш —
хадгалахгүй хаяна. Model-ийн итгэлийн түвшинг 0.75-аар хязгаарлана, учир нь өөрийнхөө ажлыг
өөрөө дүгнэх эрх model-д байхгүй.

---

## Юу хийдэг вэ

```
nexus init             хэл, framework, build систем, өгөгдлийн сан, контейнер илрүүлнэ
nexus scan             төслийг индекслээд baseline тавина
nexus rescan           baseline-тай харьцуулж → өөрчлөгдсөн симбол → нөлөөлөл
nexus impact <target>  нэг метод/файл өөрчлөгдвөл юу эвдрэхийг харуулна — бүх stack-аар
nexus graph            dependency граф хэр том, хэдэн хувь нь холбогдсон
nexus analyze [cap]    capability ажиллуулна: architect | review | bughunter
nexus findings         бүх capability-ийн олдворууд
nexus finding <id>     нэг олдвор — нотолгоо, түүхтэй нь
nexus ask <асуулт>     changed · affected X · known X · facts · next
nexus fact <key> <..>  дараагийн session-д зориулж санана
nexus doctor           тохиргоо, орчны шалгалт
nexus mcp              Claude Code, Codex, Copilot-д зориулсан MCP сервер

# төлөвлөгдсөн — V1, одоогоор баригдаагүй. docs/roadmap.md
nexus investigate      дэлгэцийн зургийн тайлбар → UI anchor → seam → сэжигтнүүд
nexus verify           reproduction тест үүсгэж, ажиллуулж, baseline дээр давтаж, дүгнэнэ
```

Бүх repo-г уншдаг цорын ганц удаа бол эхний `scan`. Дараа нь **байгаа кодын хэмжээгээр биш,
өөрчлөгдсөн хэсгийн хэмжээгээр** зардал гаргана.

### Хамгийн чухал баталгаа

109 файлтай Spring Boot төслийг бүхэлд нь дахин форматлаж үзсэн (диск дээр 14,000 мөр
өөрчлөгдсөн):

```
Changes
  109 files
  0 symbols        ← нэг ч симбол өөрчлөгдөөгүй
```

Тиймээс `spotlessApply` ажиллуулах бүрт дэмий алдаа хайж эхлэхгүй. Харин нэг методын доторх
нэг мөр өөрчлөхөд:

```
BODY_CHANGED    mn.life.wellbeing.service.WellbeingService#saveMeal(SaveMealInput)
```

Яг тэр метод. "WellbeingService.java өөрчлөгдсөн" гэсэн бүдэг хариу биш.

---

## Frontend болон backend-ийг холбоно

Хамгийн хэцүү асуулт нь: *"Backend дээрх энэ методыг өөрчилвөл frontend дээр юу эвдрэх вэ?"*

Ер нь ямар ч tool хариулж чаддаггүй, учир нь `fetch()` болон `@QueryMapping` хоёр өөр өөр
хэлэн дээрх, хоорондоо ямар ч холбоогүй хоёр функц. BugHunter тэднийг **GraphQL схемээр**
холбоно.

Autoland-ийн `sales` төсөл дээр ажиллуулсан бодит үр дүн:

```
$ nexus impact 'mn.autoland.sales.vehicle.service.VehicleService#list' --paths

  0.81  VehicleGraphQLController#vehicles(...)
  0.57  graphql:Query.vehicles
  0.46  graphql:op:Vehicles
  0.37  frontend/src/app/(sales)/vehicles/page#VehiclesPage
  0.37  frontend/src/app/(sales)/components/NewSaleModal#NewSaleModal
  0.37  frontend/src/app/(sales)/components/VehicleSelect#VehicleSelect
  …
  7 crossing the frontend/backend seam
```

Java методын нэг мөр өөрчлөхөд **зургаан React компонент** эрсдэлд орж байгааг, ямар замаар
холбогдож байгаатай нь хамт харууллаа.

Хэмжсэн үзүүлэлт: 880 файл, 5,665 симбол, төслийн дотоод хамаарлын **96% нь холбогдсон**,
641 мс.

---

## Дэлгэцийн зургаас алдаа хүртэл

Бодит амьдрал дээр алдаа ингэж ирдэг: хүн дэлгэц рүү заагаад *"энэ тоо буруу байна"* гэдэг.
Ямар коммит буруутайг нь хэн ч мэдэхгүй.

**V1-ийн зорилт.** Доорх урсгал бол зорилт, одоогийн гаралт биш — `investigate` команд
одоогоор баригдаагүй, [`docs/roadmap.md`](docs/roadmap.md)-д төлөвлөгдсөн:

```
хүн → агент     [дэлгэцийн зураг] "Сагсны нийт дүн 0 харагдаж байна, гэтэл 3 бараа байна"

  ← ТОДРУУЛГА
    Энэ хуудсан дээр 4 компонент нийт дүн харуулдаг.
    "Аль хэсэг нь буруу тоо харуулж байна — барааны жагсаалт уу,
     эсвэл доод талын нийлбэрийн хэсэг үү?"
    Яагаад асууж байна: "Хоёулаа өөр өөр endpoint дуудаж байна."

хүн → "доод талын нийлбэрийн хэсэг"

  ← МӨШГӨЛТ
    TotalsPanel → useCart() → GET /api/cart/:p → CartController#get
                → CartService#totals → CartRepository → cart_items

  ← СЭЖИГТНҮҮД
    1. CartService#totals     0.81   өчигдөр өөрчлөгдсөн · тест байхгүй
    2. CartDto.totalAmount    0.74   ТАЛБАРЫН НЭР ЗӨРСӨН
       backend `total_amount` гэж илгээж байхад frontend `totalAmount` гэж уншиж байна
       → undefined → 0 гэж харуулна
```

Хоёрдугаар сэжигтнийг **ямар ч AI model оролцоогүйгээр** оллоо. Хоёр талын талбарын нэр зөрж
байгаа, хоёулаа индекслэгдсэн — тэднийг харьцуулах нь зүгээр л join хийх ажил. Ингэснээр
агентын дүгнэлтийг үнэхээр шаардлагатай хэсэгт нь зарцуулна.

Дэлгэцийн зургийг **агент уншина, BugHunter хэзээ ч хүлээж авахгүй**. Агент зурган дээрээс
ажиглалт (маршрут, харагдаж буй текст, network хүсэлт, console алдаа) гаргаж өгөхөд
BugHunter тэднийг өөрийн индекс дээр детерминистик байдлаар холбоно.

Дэлгэрэнгүй: [`docs/investigation.md`](docs/investigation.md).

---

## Ямар ч AI агентаас ашиглана

Нэг binary, нэг MCP сервер. Агент бүрт тусад нь юм бичих шаардлагагүй.

```jsonc
// Claude Code — .mcp.json
{ "mcpServers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

```toml
# Codex — ~/.codex/config.toml
[mcp_servers.nexus]
command = "nexus"
args    = ["mcp"]
```

```jsonc
// GitHub Copilot — .vscode/mcp.json
{ "servers": { "nexus": { "command": "nexus", "args": ["mcp"] } } }
```

MCP сервер аль хэдийн ажиллаж байна: найман tool — юу өөрчлөгдсөн, юунд нөлөөлөх,
симболын хамаарал. Алдаа хайх, баталгаажуулах tool-ууд V1-д нэмэгдэнэ.

---

## Гол зарчмууд

1. **Шалгаж болох нотолгоо нь AI-н таамгаас дээр.** `file:line` заагаагүй олдвор
   хадгалагдахгүй, шууд хаягдана.
2. **AI бол заавал биш.** Детерминистик build дотор HTTP client огт байхгүй — амлалт биш,
   `cargo tree`-ээр шалгаж болох баримт.
3. **Repo-г хаашаа ч бүтнээр нь илгээхгүй.** Контекст гэдэг нь эрэмбэлэгдсэн, токены
   хязгаартай нотолгооны багц. "Файлыг бүтнээр нь оруулъя" гэсэн зам байхгүй.
4. **Продакшн кодыг өөрчлөхгүй.** Үүсгэсэн тестүүд тусдаа хавтаст амьдарна; таны working tree
   хэзээ ч checkout, stash, reset хийгдэхгүй.
5. **Алдаа чимээгүй өнгөрөхгүй.** Уншиж чадаагүй файлыг тэмдэглэж хэлнэ; тасалдсан үр дүн
   тасалдсанаа хэлнэ; baseline коммит олдохгүй бол бүтэн скан руу шилжээд **тэрийгээ хэлнэ**.
6. **`FIXED` төлөвт нотолгоо шаардана.** Хэсэгчилсэн скан дээр алдаа харагдаагүй нь тэр
   хэсгийг шалгаагүй гэсэн үг — засагдсан гэсэн үг биш.
7. **Таамаглахгүй, асууна.** Даалгавар бүрэн бус бол тодорхой асуултуудыг — тус бүрдээ
   *яагаад* асууж байгаа шалтгаантайгаар — буцаана. Дөрвөн боломжоос нэгийг нь чимээгүйхэн
   сонгоод итгэлтэй дуугарах нь хамгийн муу хувилбар: хэн ч түүнийг таамаг байсныг мэдэхгүй
   өнгөрнө.

---

## Технологи

Rust · нэг статик binary · SQLite · tree-sitter · git2.

Java, TypeScript, GraphQL одоо ажиллаж байна. Python, Rust V1-д. Хэл бүр `LanguageAnalyzer`
интерфейсийн ард байрлана. Framework мэдлэг (Spring, Next.js, Django) нь тусдаа өргөтгөлийн
цэг — Spring гэдэг Java биш шүү дээ.

| Crate | Төлөв |
|---|---|
| `nexus-types` · `nexus-store` · `nexus-vcs` · `nexus-lang` · `nexus-lang-java` · `nexus-lang-ts` · `nexus-lang-graphql` · `nexus-core` · `nexus-cli` | ажиллаж байна |
| `nexus-mcp` | ажиллаж байна — 19 tool |
| `cap-architect` · `cap-review` · `cap-bughunter` | ажиллаж байна — 3, 3, 4 дүрэм |
| `nexus-fixtures` | ажиллаж байна — benchmark fixture-үүдийг тодорхойлолтоос үүсгэнэ, дандаа адилхан sha |
| `nexus-lang-python` · `nexus-lang-rust` · `nexus-verify` | дараа |

---

## Nexus өөр дээр нь ажиллах

`make check` — CI-ийн ажиллуулдаг зүйл: fmt, clippy (warning = алдаа), бүх тест. `make fixtures`
benchmark корпусыг `target/fixtures/` дотор үүсгэнэ; `make fixtures-verify` хоёр удаа үүсгээд
нэг ч sha хөдөлсөн бол унана. `/nexus-architect` кодоос одоогийн байдлыг тогтоогоод
`docs/architecture/`-ийн төлөвлөгөөнөөс нэг даалгавар төлөвлөнө эсвэл хийнэ. Эхлээд
[`AGENTS.md`](AGENTS.md)-г унш.

---

## Баримт бичиг

Техникийн баримт бичиг англи хэл дээр байна.

| Файл | Тухай |
|---|---|
| [AGENTS.md](AGENTS.md) | агентад зориулсан танилцуулга: өөрчлөгдөшгүй дүрмүүд, санаатай сонин зүйлс, цаг үрдэг урхинууд |
| [architecture.md](docs/architecture.md) | давхаргууд, crate-үүд, модулийн хил, repo бүтэц |
| [architecture-decisions.md](docs/architecture-decisions.md) | 21 ADR — яагаад ийм болсон, өөр ямар сонголт байсан, хэзээ өөрчлөх вэ |
| [data-model.md](docs/data-model.md) | SQLite схем, 21 хүснэгт, өөрчлөгдөшгүй байдлын дүрэм |
| [change-analysis.md](docs/change-analysis.md) | өөрчлөлт хэрхэн илрүүлэх, нөлөөлөл хэрхэн тооцох, алдааны fingerprint |
| [memory-model.md](docs/memory-model.md) | төслийн санах ой: fact-ууд хэрхэн хадгалагдаж, хүчингүй болдог |
| [capabilities.md](docs/capabilities.md) | capability-ийн гэрээ, шинийг хэрхэн нэмэх вэ |
| [investigation.md](docs/investigation.md) | дэлгэцийн зургаас сэжигтэн хүртэл; frontend, backend хоёрыг холбох |
| [verification-engine.md](docs/verification-engine.md) | тест үүсгэж, ажиллуулж, дүгнэх |
| [mcp-api.md](docs/mcp-api.md) | MCP tool-ууд, зөвшөөрлийн хяналт |
| [ai-integration.md](docs/ai-integration.md) | AI хэрхэн орох вэ: агент өөрөө provider, redaction |
| [security.md](docs/security.md) | аюулгүй байдал, зөвшөөрөл, sandbox, нууц түлхүүр |
| [testing-strategy.md](docs/testing-strategy.md) | алдаа боловсруулалт, golden fixture, property тест |
| [cli-spec.md](docs/cli-spec.md) · [performance.md](docs/performance.md) · [roadmap.md](docs/roadmap.md) | CLI, гүйцэтгэл, төлөвлөгөө |
| [docs/architecture/](docs/architecture/README.md) | **Nexus юу болох ёстой вэ**: Context Engine, санах ойн амьдралын мөчлөг, баталгаажуулалт, үнэлгээ, 0–5 үе шаттай төлөвлөгөө. Төлөвлөгөө, тайлбар биш |
| [tests/fixtures/README.md](tests/fixtures/README.md) | benchmark корпус: тодорхойлолтоос детерминистик үүсгэдэг дөрвөн repo |

---

## Лиценз

MIT — [LICENSE](LICENSE).
