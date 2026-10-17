# Dev OS Package Manager

ตัวอย่าง extension พร้อม `package.json`, `bin`, ไฟล์ประกอบ และ release ที่ผูกกับ version อยู่ใน `examples/greeting-extension` ติดตั้งแพ็กเกจตัวอย่างได้ด้วย `dev install https://github.com/d-osc/DevOsPM/releases/latest/download/greeting-extension-1.0.0.dpk` แล้วเรียก `dev run extension-hello "Dev OS"` ดูรายละเอียดใน `examples/greeting-extension/README.md`

Package Manager สำหรับ **Linux** เขียนด้วย Rust ใช้คำสั่ง `dev` ติดตั้งแพ็กเกจจากไฟล์ **`.dpk` เป็นรูปแบบหลัก** และรองรับ `.tar.gz` เดิม ไม่ต้องมี daemon หรือ runtime แยกตอนใช้งาน

`.dpk` รุ่นนี้ใช้ tar + gzip ภายใน และใช้ `package.json` เป็น manifest เหมือนกันทั้งสองนามสกุล ระบบตรวจ archive, checksum, การอัปเดต และ rollback ใช้ร่วมกัน ไฟล์ที่เปลี่ยนนามสกุลเป็น `.dpk` ต้องมีเนื้อหาเป็น tar + gzip จริง

## เริ่มใช้งาน

ดาวน์โหลดไบนารี static musl สำหรับ Linux x86_64 จาก [GitHub Releases](https://github.com/d-osc/DevOsPM/releases/latest) ใช้งานได้โดยไม่ต้องติดตั้ง Rust (`dist/` ใช้เก็บผล build ภายในเครื่องและไม่อยู่ใน Git):

```sh
curl -fL https://github.com/d-osc/DevOsPM/releases/latest/download/dev-linux-x86_64 -o dev
mkdir -p "$HOME/.local/bin"
install -m 755 dev "$HOME/.local/bin/dev"
export PATH="$HOME/.local/bin:$PATH"
dev --help
```

หรือ build จาก source ด้วย Rust/Cargo 1.88 ขึ้นไป:

```sh
cargo build --release --locked
sh scripts/install.sh target/release/dev
export PATH="$HOME/.local/bin:$PATH"
```

ติดตั้งแพ็กเกจ:

```sh
dev install app.dpk
dev install "https://example.com/releases/app.dpk"
dev install "https://example.com/releases/app.tar.gz"
dev list
dev info app
dev verify app
dev remove app
```

สำหรับไฟล์ `.dpk` หรือ `.tar.gz` ที่ไม่มี manifest ชื่อเริ่มต้นมาจากชื่อไฟล์และรุ่นเป็น `0.0.0` กำหนดเองได้:

```sh
dev install app-1.2.0.dpk --name app --version 1.2.0
# รองรับแพ็กเกจเก่าด้วย
dev install legacy.tar.gz
```

ชื่อแพ็กเกจ/คำสั่งใช้ตัวพิมพ์เล็ก ตัวเลข `-` หรือ `_` ยาวไม่เกิน 64 ตัว เริ่มด้วยตัวอักษรหรือตัวเลข หากชื่อไฟล์ไม่ตรงรูปแบบ ให้ระบุ `--name`

ลิงก์ต้องเป็น HTTP(S) ที่ path ลงท้าย `.dpk` หรือ `.tar.gz` รองรับ redirect และ query string ดาวน์โหลดแบบ streaming ลงไฟล์ชั่วคราวก่อนถือ lock ของ store จำกัดขนาด 2 GiB และเวลา 10 นาที ลบไฟล์ชั่วคราวเมื่อสำเร็จหรือเกิดข้อผิดพลาด ใช้ `--sha256 <digest>` เพื่อตรวจ checksum ได้ และไม่อนุญาต redirect จาก HTTPS ไป HTTP

## รูปแบบแพ็กเกจและคำสั่ง

`homepage` เป็น string ระบุเว็บไซต์หลัก ส่วน `storepage` ใช้ไฟล์ HTML หรือ Markdown ภายในแพ็กเกจ:

```json
"storepage": {
  "index": "index.html",
  "files": ["index.html", "style.css", "images/screenshot.png"]
}
```

สำหรับ Markdown ใช้ `"index": "index.md"` และรวมไฟล์นี้ใน `files` หรือระบุแบบย่อ `"storepage": "index.md"` ก็ได้ paths ทั้งหมดอ้างอิงรากแพ็กเกจ `index` ต้องเป็น `.html`, `.htm` หรือ `.md` และอยู่ใน `files` ซึ่งห้ามซ้ำ รายการอื่นใช้สำหรับไฟล์ประกอบ เช่น CSS และรูปภาพ ตรวจว่าไฟล์มีอยู่ตอน pack/install และรวมใน `dev verify` หากไม่ระบุใช้ `""` ยังอ่าน URL string แบบเดิมได้เพื่อรองรับแพ็กเกจเก่า

ข้อมูลแสดงผ่าน `dev info` / `dev list --json` แอปร้านต้องอ่านและ render HTML/Markdown เอง Package Manager ไม่เปิดหรือรันหน้า HTML อัตโนมัติ ตัวอย่าง app ใช้ `examples/hello/store/index.html` และ extension ใช้ `examples/greeting-extension/store/index.md`

`contextmenu` เป็น array ของรายการเมนูคลิกขวาสำหรับ app และ extension:

```json
"contextmenu": [
  {
    "id": "greet-selection",
    "label": "ทักทายด้วย Greeting Extension",
    "command": "extension-hello",
    "args": ["{path}"],
    "targets": ["file", "directory"]
  }
]
```

`id` ต้องไม่ซ้ำภายในแพ็กเกจ, `command` ต้องเป็นชื่อคำสั่งที่มีใน `bin`, `targets` เลือก `file`, `directory` หรือ `background` โดยห้ามว่าง/ซ้ำ และ `args` เป็น array แยก argument ระบบเก็บข้อมูลและตรวจตอน pack/install แสดงผ่าน `dev info` / `dev list --json` หากไม่ระบุจะใช้ `[]`

ข้อมูลนี้เป็นสัญญาสำหรับ File Manager หรือแอปที่แสดงเมนู โดย host ต้องกรอง targets และแทน `{path}` ด้วย path ที่เลือก (สำหรับ background คือโฟลเดอร์ปัจจุบัน) แล้วส่ง args แยกให้ `dev run <command>` โดยตรง ไม่ผ่าน shell Package Manager รุ่นนี้ยังไม่แสดงหรือ register เมนูใน desktop อัตโนมัติ

`shortname` เป็นชื่อย่อสำหรับแสดงผล และ `fullname` เป็นชื่อเต็ม รองรับข้อความภาษาไทย โดย `name` ยังคงเป็นรหัสสำหรับ install/info/remove ไม่ใช้ชื่อแสดงผลแทนรหัส ระบบอ่าน `shotname` เป็น alias ของ `shortname` ได้ แต่ข้อมูลใหม่ใน registry ใช้ `shortname` หากไม่ระบุทั้งสองฟิลด์จะเป็น `""`

`icon` ระบุ path ของไฟล์ไอคอนภายในแพ็กเกจ เช่น `"share/icon.svg"` ใช้ได้ทั้ง app และ extension `pack` และ `install` ตรวจว่าไฟล์มีอยู่และ path ไม่ออกนอกแพ็กเกจ ไฟล์นี้รวมอยู่ใน checksum ของ `dev verify` ถ้าไม่ได้ระบุใช้ `""` ข้อมูลแสดงใน `dev info` / `dev list --json` ระบบไม่ได้แสดงไอคอนใน launcher ให้อัตโนมัติ

`repository` รองรับ object โดยเก็บ `type`, `url`, `directory` และ `release` ภายใน ใช้ `version` หลักเป็นค่ากลาง: `release.tag` มีค่าเริ่มต้น `v{version}` และ `filename` ใช้ `{name}` / `{version}` ได้ ถ้าขาดนามสกุลระบบจะเติมตาม type (`dpk` หรือ `tar.gz`)

สำหรับ GitHub HTTPS URL ระบบสร้างลิงก์ `/releases/download/<tag>/<filename>` ให้อัตโนมัติ `directory` ระบุโฟลเดอร์ซอร์สและไม่ถูกนำไปต่อกับลิงก์ไฟล์ release สำหรับผู้ให้บริการอื่นให้ระบุ `repository.release.url` ซึ่งใช้ `{name}`, `{version}`, `{tag}` และ `{filename}` ได้

`dev info` แสดง Release ที่แทนค่าแล้ว รวม version, tag, filename และ url รุ่นนี้ยังไม่มี `dev update` ใช้ URL ที่แสดงกับ `dev install "<URL>" --replace` ได้ ยังอ่าน repository string และ release string ระดับหลักจากแพ็กเกจเก่าได้ แต่ห้ามระบุ release ทั้งระดับหลักและใน repository พร้อมกัน

`bin` ใช้ object เช่น `{"hello": "bin/hello"}` หรือ string เช่น `"bin/hello"` โดยแบบ string ใช้ชื่อแพ็กเกจเป็นชื่อคำสั่ง เรียกด้วย `dev run hello` ยังอ่านฟิลด์ `commands` เดิมได้เพื่อรองรับแพ็กเกจและ registry รุ่นเก่า แต่ข้อมูลใหม่ใช้ `bin`

`package.json` ที่รากแพ็กเกจใช้ระบุประเภท:

```json
{"kind": "app"}
```

รองรับ `app` และ `extension` ถ้าไม่มีไฟล์นี้ `dev install` จะสร้างให้อัตโนมัติในแพ็กเกจที่ติดตั้ง โดยใช้ `kind: app` รวมไฟล์ใน checksum และ `dev verify` โดยไม่แก้ archive ต้นฉบับ ถ้ามีไฟล์อยู่แล้วจะเก็บเนื้อหาเดิมและตรวจว่า `kind` ถูกต้อง ไฟล์ JSON เสียหรือ kind ไม่รองรับจะติดตั้งไม่สำเร็จ ถ้าไม่ระบุ kind จะใช้ app; ถ้าไม่ระบุชื่อหรือรุ่นตอน install จะใช้ชื่อไฟล์และ 0.0.0 หรือค่าจาก --name/--version ตามลำดับ `dev pack` ต้องมีชื่อและรุ่นใน package.json ประเภทแสดงใน `dev info` และ `dev list --json`

`package.json` เป็นไฟล์กลางของ app และ extension ใช้ระบุ kind ชื่อ รุ่น และคำสั่ง รองรับ `dev-package.json` เดิมสำหรับแพ็กเกจเก่าเมื่อไม่มี `package.json` โดยสร้างไฟล์กลางจากข้อมูลเดิมให้ตอนติดตั้ง

วาง `package.json` ที่รากของ archive (ไม่ครอบด้วยโฟลเดอร์ app อีกชั้น):

```text
package.json
bin/app
share/...
```

```json
{
  "kind": "app",
  "icon": "share/icon.svg",
  "name": "app",
  "shortname": "App",
  "fullname": "My Dev OS Application",
  "homepage": "https://example.com/my-app",
  "storepage": {
    "index": "store/index.md",
    "files": ["store/index.md"]
  },
  "version": "1.2.0",
  "description": "My Dev OS application",
  "repository": {
    "type": "git",
    "url": "https://github.com/your-org/your-app.git",
    "directory": "packages/app",
    "release": {
      "type": "dpk",
      "tag": "v{version}",
      "filename": "{name}-{version}.dpk"
    }
  },
  "bin": {
    "app": "bin/app"
  }
}
```

ไฟล์ใน `bin` ต้องอยู่ในแพ็กเกจและมี executable bit ใน tar หากใช้ `dev pack` จะกำหนด executable bit ให้ไฟล์คำสั่งอัตโนมัติ ไม่รองรับ field อื่น เช่น install scripts

```sh
dev pack ./examples/hello
dev install hello-1.0.0.dpk
dev run hello -- "Dev OS"
```

`dev pack` จะสร้าง `<name>-<version>.dpk` ใน working directory เมื่อไม่ระบุ `--output` เลือกชื่อเองได้ และยังสร้าง `.tar.gz` เพื่อความเข้ากันได้:

```sh
dev pack ./examples/hello --output hello.dpk
dev pack ./examples/hello --output hello.tar.gz
```

ดาวน์โหลดแพ็กเกจตัวอย่าง `hello-1.0.0.dpk` และ `greeting-extension-1.0.0.dpk` ได้จาก GitHub Releases พร้อมไฟล์ `SHA256SUMS` สำหรับตรวจ checksum ของ artifacts

หรือสร้างด้วย tar บน Linux:

```sh
chmod +x app/bin/app
tar -czf app.dpk -C app .
dev install app.dpk
dev run app -- --help
```

`dev run` ส่ง arguments ให้โปรแกรมโดยตรง ส่งคืน exit code ของโปรแกรม และตั้ง `DEV_PACKAGE_ROOT` เป็นโฟลเดอร์แพ็กเกจ โปรแกรมเรียกใช้ไฟล์ประกอบผ่านตัวแปรนี้ได้ Working directory ยังเป็นที่ที่ผู้ใช้เรียกคำสั่ง ไฟล์แอปต้องเป็น binary/script ที่ใช้งานได้บนเครื่องนั้น

## อัปเดตและย้อนรุ่น

```sh
dev install app-1.3.0.dpk --replace
dev rollback app
dev verify
```

เก็บรุ่นที่ติดตั้งก่อนหน้าหนึ่งรุ่น `rollback` ตรวจความสมบูรณ์ของรุ่นเก่าก่อนสลับ และสามารถสลับกลับอีกรอบได้ `--replace` ยอมรับรุ่นใหม่ รุ่นเดิม หรือรุ่นเก่าที่ผู้ใช้เลือกเอง ยังไม่มีการเปรียบเทียบ semantic version

ตรวจ archive กับ SHA-256 ที่ได้รับจากผู้จัดทำแพ็กเกจ:

```sh
sha256sum app.dpk
dev install app.dpk --sha256 <trusted-64-character-sha256>
```

Checksum ช่วยตรวจว่าไฟล์ตรงกับ digest ที่เชื่อถือได้ ยังไม่มีระบบลายเซ็น/ผู้เผยแพร่ที่เชื่อถือได้ในตัว

## ความเร็วและความเสถียร

- แตก gzip และเขียนไฟล์แบบ streaming ด้วย buffer 128 KiB พร้อมคำนวณ SHA-256 ระหว่างเขียน
- ติดตั้งใน staging แล้วสลับ `registry.json` แบบ atomic เมื่อไฟล์พร้อม ใช้ `fsync` บน Linux ก่อน commit
- อัปเดตที่ล้มเหลวก่อน commit จะเก็บรุ่นเดิมไว้ หากโปรเซสถูก kill ระหว่างทำงาน อาจเหลือ staging/generation ที่ไม่ถูกอ้างอิง โดย registry ยังชี้รุ่นเดิมหรือรุ่นใหม่ที่ commit แล้ว
- ล็อก store ป้องกัน writer ทำงานพร้อมกัน คำสั่งอ่านแชร์ล็อกได้ `dev run` ถือ read lock จนโปรแกรมจบ การ install/remove ระหว่างนั้นจะตอบว่า store busy ให้ลองอีกครั้ง
- ป้องกัน path traversal, absolute paths, duplicate paths, symlink, hard link และ special files
- จำกัด 100,000 entries, payload รวม 2 GiB และ manifest 1 MiB ต่อแพ็กเกจ ตรวจ gzip CRC/trailer ก่อน commit
- รองรับ GNU long names และ local PAX metadata โดยจำกัด metadata 1 MiB; ไม่รองรับ global PAX, sparse files และ PAX size ที่ไม่ตรงกับ tar header
- `dev verify` ตรวจไฟล์หาย ไฟล์เพิ่ม ขนาด SHA-256 และ executable permissions; `dev run` ตรวจ SHA-256 ของไฟล์คำสั่งก่อนรัน

ความเร็วขึ้นกับ CPU, filesystem, จำนวนไฟล์ และ `fsync` จึงมี benchmark ที่รันซ้ำได้:

```sh
python3 scripts/benchmark.py ./target/release/dev --payload-mib 64 --files 1000
```

ผลการวัดครั้งหนึ่งบน Ubuntu/WSL บนเครื่องพัฒนา: payload 64 MiB / 1,000 ไฟล์ ติดตั้ง 5.767 วินาที, verify 0.208 วินาที, list median 14.41 ms ตัวเลขนี้เป็นผลของ workload สังเคราะห์บนเครื่องนี้ ไม่ใช่การรับประกันความเร็วบนทุกเครื่อง

## ตำแหน่งเก็บข้อมูล

ลำดับการเลือก store: `--root` → `DEV_ROOT` → `$XDG_DATA_HOME/devos` → `~/.local/share/devos`

```sh
dev --root ./sandbox install app.dpk
DEV_ROOT="$HOME/my-dev-store" dev list
dev list --json
```

แพ็กเกจอยู่แยกใน `packages/`, ไฟล์ชั่วคราวอยู่ใน `staging/`, สถานะอยู่ใน `registry.json` และล็อกอยู่ใน `.lock` ใช้ filesystem ภายในเครื่องที่รองรับ atomic rename และ file locks เก็บ store เป็นโฟลเดอร์ส่วนตัวของผู้ใช้ อย่าแก้ไขไฟล์ภายในขณะใช้งาน หาก registry เสีย โปรแกรมจะรายงานข้อผิดพลาดโดยไม่เขียนทับ

การติดตั้งอ่านไฟล์ในเครื่อง รุ่นนี้ยังไม่มี remote install/download, dependency resolver, install hooks, การติดตั้งลง `/usr` หรือการสร้างคำสั่งแอปใน PATH อัตโนมัติ ใช้ `dev run <command>` สำหรับคำสั่งที่ manifest ประกาศ plain archive ที่ไม่มี manifest ใช้ `dev info` ดูโฟลเดอร์ไฟล์ที่ติดตั้งได้

## ตรวจสอบโค้ด

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 scripts/smoke_linux.py ./target/release/dev
```

มี tests สำหรับ install/remove, plain archive, checksum, gzip เสีย/ขาด, ไฟล์และ registry ที่ถูกแก้ไข, path traversal, links, command collision, locks, commit failure, upgrade/rollback และการส่ง arguments/exit code บน Linux โค้ด CLI คอมไพล์และทดสอบบน Windows ได้ด้วย แต่แพ็กเกจ executable ต้องตรงกับระบบปฏิบัติการ

ผลตรวจรุ่น 0.3.14: unit tests ผ่าน 43 กรณีบน Linux musl และ 41 กรณีบน Windows, formatting และ Clippy บน Windows ผ่าน; ทดสอบ CLI ติดตั้งบน Linux ผ่าน ครอบคลุม .dpk, .tar.gz, checksum, upgrade/rollback, หลายโปรเซส และตัวติดตั้งไบนารี

URL installation regression test: `python3 scripts/smoke_download.py ./target/release/dev`.
