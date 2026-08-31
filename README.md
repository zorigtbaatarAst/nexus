# BugHunter

*[English](README.en.md) · Монгол*

**Кодчлолд зориулсан AI систем.** Төслийг нэг удаа уншиж индекслээд, дараа нь юу
өөрчлөгдснийг, тэр өөрчлөлт юуг хөндөж байгааг, аль хэсэгт нь алдаа гарч болзошгүйг хэлж
өгнө. Олсон алдаагаа зүгээр нэг таамаглаад орхихгүй — **тест бичиж ажиллуулж баталгаажуулна.**

> **Одоогийн байдал:** архитектур бүрэн бэлэн. `scan`, `rescan`, `impact`, `graph`, `doctor`
> ажиллаж байна. Java, TypeScript дэмжинэ. Алдаа хайх, баталгаажуулах хэсэг V1-д орно —
> [`docs/roadmap.md`](docs/roadmap.md).

---

## Суулгах

```bash
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/bughunter/main/install.sh | sh
```

Ганц файл татаж, checksum-ыг шалгаад суулгана. Өөр юу ч хэрэггүй — Java, Node, Docker
шаардахгүй.

Дараа нь ажиллуулна:

```bash
cd /таны/төслийн/зам
bughunter scan       # индекслээд baseline тавина
bughunter rescan     # юу өөрчлөгдсөн, тэр нь юунд нөлөөлөх вэ
```

`init` гэж тусад нь ажиллуулах шаардлагагүй — `scan` шаардлагатай бол өөрөө бэлдэнэ.

<details>
<summary>Бусад аргууд</summary>

```bash
# эх кодоос нь өөрөө build хийх (Rust хэрэгтэй)
curl -fsSL https://raw.githubusercontent.com/zorigtbaatarAst/bughunter/main/install.sh | sh -s -- --from-source

# тодорхой хувилбар, эсвэл өөр хавтас
... | sh -s -- --version v0.1.0 --dir ~/bin

# устгах
... | sh -s -- --uninstall

# repo-г clone хийсэн бол
make install
```
</details>

Ямар нэг зүйл болохгүй бол `bughunter doctor` ажиллуулаарай. Юу дутуу байгааг, яаж засахыг тухайн
командтай нь хамт хэлж өгнө.

---

## Гол санаа

**Нотолгоо, түүх, баталгаажуулалтыг BugHunter хариуцна. Дүгнэлтийг AI агент хариуцна.**

AI агентад таны бүх repo хэрэггүй. Түүнд хэрэгтэй нь ердөө дөрвөн зүйл: эх кодын зөв хэсэг,
юу өөрчлөгдсөн, тэр өөрчлөлт юуг хөндөж байгаа, мөн тэр газар өмнө нь юу эвдэрч байсан.
BugHunter-ийн бүх ажил бол яг үүнийг найдвартай бэлдэж өгөх.

Ингэснээр MCP дэмждэг ямар ч агент ашиглаж чадна — Claude Code, Codex, Copilot, эсвэл
хараахан гараагүй ямар нэг зүйл. Мөн AI-г бүр мөсөн унтраасан ч ажиллана: `scan`, `rescan`,
`impact` бүгд ямар ч model, API key шаардахгүй.

---

## Юу хийдэг вэ

```
bughunter scan         төслийг индекслээд baseline тавина
bughunter rescan       baseline-тай харьцуулж → өөрчлөгдсөн симбол → нөлөөлөл
bughunter impact       нэг метод/файл өөрчлөгдвөл юу эвдрэхийг харуулна
bughunter graph        dependency граф хэр том, хэдэн хувь нь холбогдсон
bughunter hunt         детерминистик шалгуурууд ажиллуулах
bughunter bugs         олдворуудын жагсаалт
bughunter bug <id>     нэг олдвор — нотолгоо, түүхтэй нь
bughunter mcp          Claude Code, Codex, Copilot-д зориулсан MCP сервер
bughunter doctor       тохиргоо, орчны шалгалт
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
$ bughunter impact 'mn.autoland.sales.vehicle.service.VehicleService#list' --paths

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
{ "mcpServers": { "bughunter": { "command": "bughunter", "args": ["mcp"] } } }
```

```toml
# Codex — ~/.codex/config.toml
[mcp_servers.bughunter]
command = "bughunter"
args    = ["mcp"]
```

```jsonc
// GitHub Copilot — .vscode/mcp.json
{ "servers": { "bughunter": { "command": "bughunter", "args": ["mcp"] } } }
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

Java, TypeScript одоо ажиллаж байна. Python, Rust V1-д. Хэл бүр `LanguageAnalyzer`
интерфейсийн ард байрлана. Framework мэдлэг (Spring, Next.js, Django) нь тусдаа өргөтгөлийн
цэг — Spring гэдэг Java биш шүү дээ.

| Crate | Төлөв |
|---|---|
| `bh-types` · `bh-store` · `bh-vcs` · `bh-lang` · `bh-lang-java` · `bh-lang-ts` · `bh-core` · `bh-cli` | ажиллаж байна |
| `bh-mcp` | ажиллаж байна — найман tool |
| `bh-lang-python` · `bh-lang-rust` · `bh-verify` · `bh-ai` | V1 |

---

## Баримт бичиг

Техникийн баримт бичиг англи хэл дээр байна.

| Файл | Тухай |
|---|---|
| [architecture.md](docs/architecture.md) | давхаргууд, crate-үүд, модулийн хил, repo бүтэц |
| [architecture-decisions.md](docs/architecture-decisions.md) | 17 ADR — яагаад ийм болсон, өөр ямар сонголт байсан, хэзээ өөрчлөх вэ |
| [data-model.md](docs/data-model.md) | SQLite схем, 21 хүснэгт, өөрчлөгдөшгүй байдлын дүрэм |
| [change-analysis.md](docs/change-analysis.md) | өөрчлөлт хэрхэн илрүүлэх, нөлөөлөл хэрхэн тооцох, алдааны fingerprint |
| [investigation.md](docs/investigation.md) | дэлгэцийн зургаас сэжигтэн хүртэл; frontend, backend хоёрыг холбох |
| [verification-engine.md](docs/verification-engine.md) | тест үүсгэж, ажиллуулж, дүгнэх |
| [mcp-api.md](docs/mcp-api.md) | MCP tool-ууд, зөвшөөрлийн хяналт |
| [security.md](docs/security.md) | аюулгүй байдал, зөвшөөрөл, sandbox, нууц түлхүүр |
| [cli-spec.md](docs/cli-spec.md) · [performance.md](docs/performance.md) · [roadmap.md](docs/roadmap.md) | CLI, гүйцэтгэл, төлөвлөгөө |

---

## Лиценз

MIT — [LICENSE](LICENSE).
