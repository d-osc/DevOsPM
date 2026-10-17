# Greeting Extension

หน้าแนะนำในร้านเป็น Markdown ที่ `store/index.md` ระบุใน `storepage.index` พร้อม `storepage.files` รวมหน้าและไอคอนที่ใช้ แสดงผลผ่านแอปร้านที่รองรับ (Package Manager ไม่ render หน้าเอง)

มี `contextmenu` ตัวอย่างสำหรับไฟล์และโฟลเดอร์ อ้างอิง `extension-hello` ใน `bin` พร้อม `args: ["{path}"]` ข้อมูลนี้ให้ File Manager อ่านไปสร้างเมนูและแทน path ก่อนเรียกคำสั่ง Package Manager เก็บและตรวจข้อมูลให้ แต่ยังไม่เพิ่มเมนูใน desktop อัตโนมัติ

ระบุไอคอนผ่าน `"icon": "share/icon.svg"` ใน `package.json` ไฟล์ SVG นี้แนบอยู่ในแพ็กเกจและถูกตรวจด้วย `dev verify`

ตัวอย่างแพ็กเกจ `kind: extension` ที่เพิ่มคำสั่ง `extension-hello` ให้ Dev OS Package Manager โดยอ่านไฟล์ประกอบผ่าน `DEV_PACKAGE_ROOT` ใช้ `/bin/sh` บน Linux ไม่ต้องติดตั้ง runtime เพิ่ม

จากรากโปรเจกต์:

```sh
dev pack examples/greeting-extension --output greeting-extension-1.0.0.dpk
dev install greeting-extension-1.0.0.dpk
dev info greeting-extension
dev run extension-hello "Dev OS"
dev verify greeting-extension
dev remove greeting-extension
```

ผลคำสั่ง:

```text
Hello from greeting-extension, Dev OS!
Extension directory: <โฟลเดอร์แพ็กเกจที่ติดตั้ง>
```

ดาวน์โหลดไฟล์ตัวอย่างจาก GitHub Releases ด้วย `dev install https://github.com/d-osc/DevOsPM/releases/latest/download/greeting-extension-1.0.0.dpk` ได้เลย

`repository.url` เป็นค่าตัวอย่าง ให้เปลี่ยนเป็น repository ของคุณก่อนเผยแพร่ ตัวอย่างนี้ไม่ได้อัปโหลด release จริง ระบบแทนค่า `version: 1.0.0` เป็น tag `v1.0.0` และ filename `greeting-extension-1.0.0.dpk`

ตัวอย่างนี้สาธิตการติดตั้งแพ็กเกจประเภท extension และการ export คำสั่งผ่าน `bin` การโหลด extension เข้าแอปต้องมีระบบรองรับในแอปนั้นเอง
