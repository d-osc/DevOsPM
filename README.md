# Dev OS Package Manager

ตัวอย่าง extension พร้อม `package.json`, `bin`, ไฟล์ประกอบ และ release ที่ผูกกับ version อยู่ใน `examples/greeting-extension` ติดตั้งแพ็กเกจตัวอย่างได้ด้วย `dev install https://github.com/d-osc/DevOsPM/releases/latest/download/greeting-extension-1.0.0.dpk` แล้วเรียก `dev run extension-hello "Dev OS"` ดูรายละเอียดใน `examples/greeting-extension/README.md`

Package Manager สำหรับ **Linux** เขียนด้วย Rust ใช้คำสั่ง `dev` ติดตั้งแพ็กเกจจากไฟล์ **`.dpk` เป็นรูปแบบหลัก** รองรับ `.tar.gz` เดิม และติดตั้งจาก `.deb` / `.rpm` ได้ ไม่ต้องมี daemon หรือ runtime แยกตอนใช้งาน รุ่นนี้ยังเชื่อมกับ **Dev OS desktop** โดยตรง: ติดตั้งแพ็กเกจ extension ของ Extension Store, สร้างรายการแอปให้ Launcher และอัปเดตจาก GitHub Releases ด้วย `dev update` (ดูหัวข้อ "เชื่อมกับ Dev OS desktop")

`.dpk` รุ่นนี้ใช้ tar + gzip ภายใน และใช้ `package.json` เป็น manifest เหมือนกันทั้งสองนามสกุล ระบบตรวจ archive, checksum, การอัปเดต และ rollback ใช้ร่วมกัน ไฟล์ที่เปลี่ยนนามสกุลเป็น `.dpk` ต้องมีเนื้อหาเป็น tar + gzip จริง

## เริ่มใช้งาน

ดาวน์โหลดไบนารี static musl สำหรับ Linux **x86_64** หรือ **arm64** จาก [GitHub Releases](https://github.com/d-osc/DevOsPM/releases/latest) ใช้งานได้โดยไม่ต้องติดตั้ง Rust (`dist/` ใช้เก็บผล build ภายในเครื่องและไม่อยู่ใน Git):

```sh
case "$(uname -m)" in aarch64) ARCH=arm64;; *) ARCH=x86_64;; esac
curl -fL "https://github.com/d-osc/DevOsPM/releases/latest/download/dev-linux-$ARCH" -o dev
mkdir -p "$HOME/.local/bin"
install -m 755 dev "$HOME/.local/bin/dev"
export PATH="$HOME/.local/bin:$PATH"
dev --help
```

หรือติดตั้งด้วยแพ็กเกจ `.deb` / `.rpm` (ติดตั้งไบนารีที่ `/usr/bin/dev`):

```sh
# Debian / Ubuntu
sudo apt install ./dev-os-package-manager_<version>_amd64.deb      # หรือ _arm64.deb
# Fedora / RHEL / openSUSE
sudo dnf install ./dev-os-package-manager-<version>-1.x86_64.rpm   # หรือ .aarch64.rpm
```

ผู้ดูแลสร้างแพ็กเกจได้ด้วย [nfpm](https://nfpm.goreleaser.com/install/): รัน `sh scripts/build_release.sh` แล้วตามด้วย `sh scripts/package_linux.sh` ผลลัพธ์อยู่ที่ `dist/packages/` (config อยู่ที่ `packaging/nfpm.yaml`) หมายเหตุ: `dev update` แทนที่ไบนารีที่รันอยู่ ถ้าติดตั้งที่ `/usr/bin` ต้องรันด้วยสิทธิ์ root หรืออัปเดตผ่าน package manager ของระบบ

หรือ build จาก source ด้วย Rust/Cargo 1.88 ขึ้นไป:

```sh
cargo build --release --locked
sh scripts/install.sh target/release/dev
export PATH="$HOME/.local/bin:$PATH"
```

### รุ่น DevLang

นอกจากไบนารี Rust ยังมี **implementation ด้วย DevLang** อยู่ใน [`devlang/`](devlang/) ใช้ runtime `d` (≥ 0.5.5) แทนไบนารีแยก คำสั่ง ตำแหน่งไฟล์ registry และ layout ของ store เหมือนรุ่น Rust ทุกประการ ทั้งสองรุ่นอ่าน store ของกันและกันได้ (ทดสอบ cross-install ทั้งสองทิศทางแล้ว):

```sh
sh devlang/install.sh    # ต้องมี d บน PATH ก่อน; ติดตั้งไป ~/.local
dev --help
```

รุ่นนี้ `dev self-update` สลับ **ชุด source `.dev`** จาก asset `dev-cli.tar.gz` ของ release ล่าสุด (ไม่ใช่ไบนารีเดี่ยวแบบรุ่น Rust) รายละเอียดความต่างอื่นๆ (เช่น `.rpm` ยังไม่รองรับในรุ่น DevLang, `dev run` แสดงผลหลังคำสั่งจบ) อยู่ใน [devlang/README.md](devlang/README.md)

ติดตั้งแพ็กเกจ:

```sh
dev install app.dpk        # หรือ dev add|i app.dpk
dev install "https://example.com/releases/app.dpk"
dev install "https://example.com/releases/app.tar.gz"
dev list
dev info app
dev verify app
dev remove app        # หรือ dev uninstall|un|rm app
dev update
```

สำหรับไฟล์ `.dpk` หรือ `.tar.gz` ที่ไม่มี manifest ชื่อเริ่มต้นมาจากชื่อไฟล์และรุ่นเป็น `0.0.0` กำหนดเองได้:

```sh
dev install app-1.2.0.dpk --name app --version 1.2.0
# รองรับแพ็กเกจเก่าด้วย
dev install legacy.tar.gz
```

ชื่อแพ็กเกจ/คำสั่งใช้ตัวพิมพ์เล็ก ตัวเลข `-` หรือ `_` ยาวไม่เกิน 64 ตัว เริ่มด้วยตัวอักษรหรือตัวเลข หากชื่อไฟล์ไม่ตรงรูปแบบ ให้ระบุ `--name`

ลิงก์ต้องเป็น HTTP(S) ที่ path ลงท้าย `.dpk`, `.dpkx`, `.tar.gz`, `.deb` หรือ `.rpm` รองรับ redirect และ query string ดาวน์โหลดแบบ streaming ลงไฟล์ชั่วคราวก่อนถือ lock ของ store จำกัดขนาด 2 GiB และเวลา 10 นาที ลบไฟล์ชั่วคราวเมื่อสำเร็จหรือเกิดข้อผิดพลาด ใช้ `--sha256 <digest>` เพื่อตรวจ checksum ได้ และไม่อนุญาต redirect จาก HTTPS ไป HTTP

## ติดตั้งจากไฟล์ .deb / .rpm

`dev install` อ่านแพ็กเกจ Debian และ RPM ได้โดยตรง ไม่ต้องมี `dpkg`/`rpm` และไม่ต้องใช้ root:

```sh
dev install hello-tool_2.3_amd64.deb
dev install "https://example.com/releases/hello-tool-2.3-1.x86_64.rpm" --sha256 <digest>
dev run hello-tool
```

ไฟล์จะถูกแตกลง store ของ `dev` (ไม่ใช่ `/usr`) ผ่านขั้นตอนตรวจ path/ขนาด/checksum และมี `dev verify`, `--replace`, `dev rollback`, `dev remove` เหมือน `.dpk`:

- ชื่อ รุ่น และคำอธิบายอ่านจาก `control` ของ deb หรือ header ของ rpm (`--name` / `--version` ใช้แทนได้) รุ่นถูกแปลงให้ใช้ได้กับ Dev OS เช่น `1:2.3-1~u1` เป็น `1.2.3-1.u1` และ rpm ใช้ `version-release`
- คำสั่งถูกสร้างอัตโนมัติจากไฟล์ที่รันได้ใน `usr/bin`, `usr/sbin`, `usr/local/bin`, `bin`, `sbin` (ชื่อที่ไม่ผ่านกติกาชื่อคำสั่งจะถูกข้าม)
- รองรับ payload แบบ gzip, xz และ zstd (ไม่รองรับ bzip2)
- **ไม่** รัน maintainer scripts, ไม่แก้ dependency และไม่สร้าง symlink/hard link/ไฟล์พิเศษ (จะข้ามและแจ้งเตือนจำนวนที่ข้าม) ดังนั้นไบนารีที่ลิงก์แบบ dynamic ต้องมีไลบรารีในระบบอยู่แล้ว เหมาะกับแพ็กเกจแบบ self-contained หรือเครื่องมือ CLI ง่าย ๆ
- ใช้ `--no-desktop` ถ้าไม่ต้องการสร้างรายการ launcher และ shim ให้ทุกคำสั่งที่ตรวจพบ

### เรียกคำสั่งได้ตรงๆ (`code` แทน `dev run code`)

ตอนติดตั้งแอป `dev` สร้าง shim สคริปต์เล็กๆ ชื่อเดียวกับคำสั่งไว้ข้างไบนารี `dev` (ซึ่งอยู่ใน PATH อยู่แล้ว เช่น `/usr/bin`) หรือที่ `$DEV_BIN_DIR` ถ้าตั้งไว้ จึงพิมพ์ `code` ได้เหมือน `dev run code`:

- shim มีบรรทัด `# dev-shim: package=<ชื่อ>` `dev` จะเขียนทับ/ลบเฉพาะไฟล์ที่มีบรรทัดนี้ ไม่แตะไฟล์ของระบบ ถ้าชื่อซ้ำกับไฟล์อื่น (เช่น `/usr/bin/code` ที่ติดตั้งด้วย apt) จะข้ามพร้อมแจ้งเตือนและยังใช้ `dev run <คำสั่ง>` ได้
- ต้องมีสิทธิ์เขียนโฟลเดอร์นั้น ถ้า `dev` อยู่ที่ `/usr/bin` ให้ติดตั้งด้วย `sudo` หรือตั้ง `DEV_BIN_DIR=$HOME/.local/bin`
- อัปเกรด, rollback และ `dev remove` ปรับหรือลบ shim ให้ตามคำสั่งของรุ่นที่ใช้งานอยู่

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

`dev info` แสดง Release ที่แทนค่าแล้ว รวม version, tag, filename และ url ใช้ `dev update` เพื่อตรวจรุ่นล่าสุดจาก GitHub แล้วติดตั้งแทนอัตโนมัติ ยังอ่าน repository string และ release string ระดับหลักจากแพ็กเกจเก่าได้ แต่ห้ามระบุ release ทั้งระดับหลักและใน repository พร้อมกัน

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

`dev pack` จะสร้าง `<name>-<version>.dpk` ใน working directory เมื่อไม่ระบุ `--output` เลือกชื่อเองได้ และยังสร้าง `.tar.gz` เพื่อความเข้ากันได้ (นามสกุล `.dpkx` ใช้ได้ด้วยหากต้องการทำเครื่องหมายว่าเป็นแพ็กเกจสำหรับ Dev OS extension):

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

## เชื่อมกับ Dev OS desktop

`dev` ทำงานร่วมกับ desktop ของ [Dev OS](https://github.com/d-osc/DevOs) ได้โดยตรงในสามจุด

### ติดตั้ง extension ของ Extension Store จาก CLI

`dev install` รับแพ็กเกจ extension ของ Dev OS ซึ่งเป็น tar ที่ทุกไฟล์อยู่ใต้ `extension/` และมี `extension.json` เป็น manifest ชื่อไฟล์ลงท้าย `.dpk`, `.dpkx` หรือ `.tar.gz` ก็ได้ เพราะระบบดูชนิดแพ็กเกจจากเนื้อหาใน tar (asset มาตรฐานของ Dev OS ชื่อ `dev-os-extension.tar.gz`):

```sh
dev install dev-os-extension.tar.gz
dev install "https://github.com/owner/repo/releases/download/v1.0.0/dev-os-extension.tar.gz"
dev list                      # แสดงส่วน dev-os extension
dev info org.example.status
dev rollback org.example.status
dev remove org.example.status --purge
```

ระบบติดตั้งลง layout เดียวกับที่หน้า **Settings → Extension Store** ของ Dev OS ใช้: release directory ที่ `~/.local/share/dev-os/extension-store/releases/<id>/<version>-<digest12>/` พร้อมไฟล์ `store-install.json` และ symlink ที่ `~/.local/share/dev-os/extensions/<id>` ชี้มายังรุ่นที่เลือก ดังนั้น:

- เปิด session ใหม่ของ Dev OS แล้ว extension ถูกโหลดเหมือนติดตั้งผ่าน Store
- หน้า Store ยังอัปเดต/ย้อนรุ่นต่อได้เพราะ format ตรงกัน (ต้อง restart session เพื่อโหลดโค้ดเหมือนเดิม)
- ติดตั้งจาก URL ของ github.com ระบบจดจำ `owner/repo` ไว้ในประวัติติดตั้ง ส่วนไฟล์ในเครื่องใช้ `local/cli` และ CLI ไม่แก้ไข `repositories.json` ของ Store เพื่อไม่ให้ session ที่กำลังเปิดอยู่ทับข้อมูล

การตรวจสอบตามกติกาของ Dev OS: id ต้องเป็น namespace ตัวพิมพ์เล็กเช่น `org.example.status`, `apiVersion` ต้องเป็น 1, `version` เป็น semver เต็ม, `kind: ui` ต้องมีไฟล์ `dist/<entry>` จริง, ห้าม `system: true`, ห้าม id ขึ้นต้น `org.devos.` และห้ามใช้ชื่อ entry ที่สงวนไว้ของ shared runtime ขนาด tar ไม่เกิน 32 MiB, แตกแล้วไม่เกิน 128 MiB และไม่เกิน 4,000 entries `dev verify` ตรวจ checksum ของไฟล์ extension เหมือนแพ็กเกจทั่วไป `dev remove` ลบแค่ symlink และเก็บ release directories กับ preferences ไว้เหมือน Store (ใช้ `--purge` เพื่อลบ release directories ด้วย) การสร้าง symlink ต้องรันบน Linux; บน Windows ต้องเปิด Developer Mode

ตำแหน่งข้อมูล Dev OS ปรับได้ด้วย `--devos-data <dir>` (ค่าเริ่มต้น `$XDG_DATA_HOME/dev-os`)

### แอปปรากฏใน Launcher

แพ็กเกจ `kind: app` ที่ติดตั้งจะได้ไฟล์ `.desktop` ที่ `~/.local/share/applications/dev-<คำสั่ง>.desktop` โดยอัตโนมัติ Launcher และ panel ของ Dev OS อ่านตำแหน่งนี้ผ่าน GIO และรีเฟรชทันทีที่ไฟล์เปลี่ยน แอปจึงโผล่ใน Launcher ทันทีโดยไม่ต้อง restart session ชื่อแสดงใช้ `fullname` หรือ `shortname`, คำอธิบายใช้ `description`, ไอคอนใช้ไฟล์จาก `icon` และคำสั่งรันคือ `<ตัว dev> run <คำสั่ง>` ระบบลบ/สร้างรายการใหม่ให้ตอน upgrade, rollback และ remove ใช้ `dev install --no-desktop` เพื่อไม่สร้างไฟล์นี้ (เช่นเครื่อง headless) และ `--applications <dir>` เปลี่ยนไดเรกทอรีปลายทาง

### dev update

```sh
dev update              # ทุกแพ็กเกจ/extension ที่ตั้งค่า release ไว้
dev update hello        # เฉพาะที่ระบุ
```

สำหรับแพ็กเกจที่มี `repository.release` ระบบอ่าน release เสถียรล่าสุดจาก GitHub API (`releases/latest`), ปฏิเสธ draft/prerelease, เลือก asset ตาม template `filename` ด้วยรุ่นใหม่, ตรวจว่า asset URL อยู่ใน release นั้นจริง ตรวจ SHA-256 กับ `digest` จาก API แล้วติดตั้งแทนรุ่นเดิมแบบทรานแซกชัน ถ้ารุ่นติดตั้งยังใหม่กว่าหรือเท่ากันจะข้ามพร้อมข้อความ up to date extension ที่ติดตั้งจาก GitHub repo จะอัปเดตผ่าน asset `dev-os-extension.tar.gz` โดยเทียบรุ่นจาก `extension.json` ของ release ก่อนดาวน์โหลด ใช้ env `DEV_UPDATE_API` เพื่อชี้ API mirror สำหรับทดสอบ

### dev self-update

```sh
dev self-update
```

อัปเดตตัว `dev` เองจาก release ล่าสุดของ [d-osc/DevOsPM](https://github.com/d-osc/DevOsPM/releases): เลือกไบนารี static ตามสถาปัตยกรรมปัจจุบัน (`dev-linux-x86_64` หรือ `dev-linux-arm64`), ตรวจ SHA-256 กับ digest จาก API ระหว่างดาวน์โหลด, **ทดสอบรันไบนารีใหม่ด้วย `--version` ก่อน** ว่ารายงานรุ่นถูกต้อง แล้วจึงแทนที่ไฟล์ที่รันอยู่ด้วย atomic rename ถ้ารุ่นปัจจุบันใหม่กว่าหรือเท่ากันจะแจ้ง up to date และไม่แตะไฟล์ คำสั่งนี้ไม่ถือ lock ของ package store รันคู่กับคำสั่งอื่นได้

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

การติดตั้งอ่านไฟล์ในเครื่อง รุ่นนี้ยังไม่มี dependency resolver, install hooks หรือการติดตั้งลง `/usr` คำสั่งแอปเข้าถึงผ่าน `dev run <command>` และ `dev update` ดึงแพ็กเกจจาก GitHub Releases ตามที่ manifest ประกาศ plain archive ที่ไม่มี manifest ใช้ `dev info` ดูโฟลเดอร์ไฟล์ที่ติดตั้งได้

## ตรวจสอบโค้ด

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
python3 scripts/smoke_linux.py ./target/release/dev
```

มี tests สำหรับ install/remove, plain archive, checksum, gzip เสีย/ขาด, ไฟล์และ registry ที่ถูกแก้ไข, path traversal, links, command collision, locks, commit failure, upgrade/rollback และการส่ง arguments/exit code บน Linux รวมถึง extension ของ Dev OS, รายการ Launcher และการแยก asset ของ `dev update` โค้ด CLI คอมไพล์และทดสอบบน Windows ได้ด้วย แต่แพ็กเกจ executable และการ symlink ของ extension ต้องรันบน Linux

ผลตรวจรุ่น 0.5.0 บน Ubuntu 24.04 / WSL2 (ARM64): unit tests ผ่าน 55 กรณี, formatting และ Clippy ผ่าน; smoke test ครอบคลุม .dpk, .dpkx, .tar.gz, checksum, upgrade/rollback, หลายโปรเซส, ตัวติดตั้งไบนารี, launcher entries, extension ของ Dev OS, `dev update` และ `dev self-update` ผ่าน API mirror ในเครื่อง

URL installation regression test: `python3 scripts/smoke_download.py ./target/release/dev`.
