<div align="center">

# 🖱️ clickshift

**An X-Mouse Button Control-style app, rebuilt in Rust for GNOME on Wayland**

Remap mouse buttons and run timed macros, with different settings for each app.

![Rust](https://img.shields.io/badge/Rust-2024-b7410e?logo=rust&logoColor=white)
![GNOME](https://img.shields.io/badge/GNOME_Shell-51-4a86cf?logo=gnome&logoColor=white)
![Wayland](https://img.shields.io/badge/Wayland-supported-ffbc00?logo=wayland&logoColor=black)
![License](https://img.shields.io/badge/License-MIT-2ea44f)

**English** · [Tiếng Việt](#tieng-viet) · [简体中文](#zh-cn)

![The clickshift settings app](docs/screenshot.png)

</div>

<a id="english"></a>

clickshift is a similar app to [X-Mouse Button Control](https://www.highrez.co.uk/downloads/XMouseButtonControl.htm)
on Windows, rebuilt from scratch in Rust to run on GNOME Wayland. It shares no code
with X-Mouse Button Control and is not affiliated with it.

## ✨ Features

### Buttons and wheel

- 🎯 Turn a mouse button into a key, a key combination, a media key or another mouse button
- 🖲️ Reverse the wheel, or make each wheel notch press a key
- 🚀 Run a shell command with one click
- 🚫 Disable a button, or leave it unchanged

### Per-app profiles

- 🪟 Give each app its own profile, which switches automatically with the focused window
- 🗂️ Anything a profile leaves out falls back to your general settings

### Advanced actions

- ⏱️ Do one thing on a quick click and another on a long press
- 🤝 Press two buttons together for a separate action
- 📌 Sticky buttons: click once to hold, click again to release, handy for dragging
- 🧱 Add up to 9 extra layers, used while a button is held or toggled on and off

### Timed macros

- 🎬 Chain keys and clicks with waits set in milliseconds
- 🔁 Three modes: play while held, repeat while held, or click to start and stop
- 🛟 Stops and releases everything when the focused window changes or the mouse disconnects

### Settings app

- 🎨 GTK 4 and libadwaita interface, so you never have to edit the config by hand
- 🌐 In English, Vietnamese and Simplified Chinese, with a language picker in the app
- ⌨️ Record a key by pressing it on the keyboard
- 💾 Changes apply within about a second, with no Apply button
- 🧾 Keeps the comments in your config and refuses invalid values
- 🔌 Turn the background service on or off from the app

### Stability and safety

- ♻️ Reconnects by itself when the mouse or its receiver comes back
- 🧯 The kernel returns the mouse whenever the program ends, even after a crash
- 🔒 The udev rule covers only the mouse you name, and does not expose your keyboard
- 🛑 The left button is locked by default, so you cannot lose control of the pointer

## ⚙️ How it works

```mermaid
flowchart LR
    A[Real mouse] -->|evdev grab| B[clickshift]
    B -->|uinput| C[Virtual mouse and keyboard]
    C --> D[GNOME and your apps]
    E[GNOME Shell extension] -->|D-Bus: focused app| B
    F[clickshift-settings] -->|config.toml| B
```

## 📦 Install

You need Rust and the GTK 4 and libadwaita development files. On Arch or CachyOS:

```sh
sudo pacman -S --needed rust gtk4 libadwaita
```

Run every `sudo` command in a real terminal, because it asks for your password.

**1. Build it.**

```sh
git clone https://github.com/1Tyson/clickshift.git
cd clickshift
cargo build --release --workspace
```

**2. Give your user access to the mouse.** The rule is written for an `AJAZZ 2.4G 8K`,
so first change the name in `packaging/70-clickshift.rules` to match your mouse.
`sudo evtest` lists the names of your input devices.

```sh
sudo install -m 644 packaging/70-clickshift.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input --action=change
target/release/clickshift list
```

The last command should show your mouse. If it does not, unplug and replug the receiver.

**3. Create the config.** Set `name` under `[device]` to your mouse's name.

```sh
mkdir -p ~/.config/clickshift
cp config.example.toml ~/.config/clickshift/config.toml
```

**4. Install the GNOME extension.** It tells clickshift which app is focused. Copy it,
log out and back in, then enable it.

```sh
cp -r extension/clickshift@clickshift.github.io ~/.local/share/gnome-shell/extensions/
```

```sh
gnome-extensions enable clickshift@clickshift.github.io
```

## ▶️ Run it

**Try it first.** The first command changes nothing, and the second gives the mouse back
after 30 seconds.

```sh
target/release/clickshift run --dry-run --verbose
target/release/clickshift run --grab-seconds 30 --verbose
```

Press Ctrl+C on the keyboard to stop either one early.

**Keep it running.** This starts clickshift now and on every login.

```sh
install -D target/release/clickshift ~/.local/bin/clickshift
install -Dm 644 packaging/clickshift.service ~/.config/systemd/user/clickshift.service
systemctl --user daemon-reload
systemctl --user enable --now clickshift.service
```

**Open the settings app.** Start it directly:

```sh
target/release/clickshift-settings
```

To open it from the app menu instead, install it once and search for **Clickshift**:

```sh
install -D target/release/clickshift-settings ~/.local/bin/clickshift-settings
install -Dm 644 packaging/io.github.clickshift.Settings.desktop ~/.local/share/applications/io.github.clickshift.Settings.desktop
```

**Manage the service.**

```sh
systemctl --user status clickshift.service         # is it running?
journalctl --user -u clickshift.service -f         # follow its log
systemctl --user restart clickshift.service        # restart it
systemctl --user disable --now clickshift.service  # stop it and turn off autostart
```

After rebuilding, run the first `install` line above again and restart the service.

Do not run clickshift alongside an input-remapper preset on the same mouse, since only
one program can hold it at a time.

## 🎬 Macros

Macros use the familiar X-Mouse Button Control style:

```toml
[apps.steam_proton]
side = "macro-hold:e{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}"
```

To add one in the settings app:

1. Pick an app on the left, or use **+** to add one.
2. Set a button to **Macro**, and choose how it plays.
3. Click **Edit macro**, paste your macro, and click **Save**.

| Prefix | Behavior |
|---|---|
| `macro-hold:` | Plays once while held, and stops when you let go |
| `macro-repeat:` | Repeats while held, and stops when you let go |
| `macro:` | Click to start, click again to stop |

| Token | Meaning |
|---|---|
| `e`, `q`, `w` | Type a key, assuming a US keyboard layout |
| `{LMB}` `{RMB}` `{MMB}` `{MB4}` `{MB5}` | Click; add `D` or `U` to press or release, as in `{LMBD}` |
| `{WAITMS:160}` | Wait 160 milliseconds |
| `{MWUP}` `{MWDN}` `{MWLT}` `{MWRT}` | Scroll one notch |
| `{Enter}` `{F5}` `{Ctrl+C}` | Tap a key or key combination |
| `{PRESS:Ctrl}` `{RELEASE:Ctrl}` | Press or release a key |

Tokens not listed here are refused with an error. Other X-Mouse Button Control tokens,
such as `{WAIT:1}`, `{WAITMS:10-20}` and `{CTRL}a`, are not supported yet. See
[`config.example.toml`](config.example.toml) for the full config syntax.

> [!WARNING]
> Many online games forbid automated macros. Check a game's terms before using one.

## 🧪 Tests

```sh
cargo test    # logic only, needs no permissions
```

The end-to-end suite runs the real binary against a virtual mouse. It grabs the output
devices before sending anything, so no input reaches the desktop, and it never touches
the real mouse. It needs one extra udev rule:

```sh
sudo install -m 644 packaging/71-clickshift-test.rules /etc/udev/rules.d/
sudo udevadm control --reload
cargo test --test e2e -- --ignored
```

## ⚠️ Limits

- Only tested on CachyOS with GNOME Shell 51 on Wayland.
- Per-app profiles need GNOME, and `metadata.json` lists Shell 51 only. Elsewhere, every
  app uses the default profile.
- Profiles follow the focused window, not the window under the pointer.
- One mouse per config.
- Command-line and config error messages are in English only.
- Macro characters assume a US keyboard layout.
- A macro stops when focus changes, so one cannot switch windows by itself.
- Wheel directions cannot use macros, sticky buttons, held layers or tap and hold.
- The settings app cannot unlock the left button; set `allow_left_remap` by hand.

## 📄 License

[MIT](LICENSE)

---

<a id="tieng-viet"></a>

# 🇻🇳 Tiếng Việt

[English](#english) · **Tiếng Việt** · [简体中文](#zh-cn)

clickshift là ứng dụng tương tự [X-Mouse Button Control](https://www.highrez.co.uk/downloads/XMouseButtonControl.htm)
trên Windows, được viết lại từ đầu bằng Rust để chạy trên GNOME Wayland. Dự án không dùng
chung mã nguồn và không liên kết với X-Mouse Button Control.

## ✨ Làm được gì

### Gán nút và con lăn

- 🎯 Đổi nút chuột thành phím, tổ hợp phím, phím media hoặc nút chuột khác
- 🖲️ Đổi hướng con lăn, hoặc biến mỗi nấc cuộn thành một phím
- 🚀 Chạy lệnh shell chỉ bằng một nút bấm
- 🚫 Tắt hẳn một nút, hoặc giữ nguyên chức năng gốc của nó

### Theo từng ứng dụng

- 🪟 Mỗi app có profile riêng, tự đổi khi bạn chuyển cửa sổ
- 🗂️ Nút không được đặt trong profile sẽ dùng thiết lập chung

### Thao tác nâng cao

- ⏱️ Bấm nhanh một việc, giữ lâu một việc khác
- 🤝 Bấm hai nút cùng lúc để kích hoạt hành động riêng
- 📌 Nút dính: bấm một lần để giữ, bấm lần nữa để nhả, tiện khi kéo thả
- 🧱 Thêm tối đa 9 lớp phím, dùng bằng cách giữ nút hoặc bật/tắt

### Macro có hẹn giờ

- 🎬 Ghi chuỗi phím và click kèm thời gian chờ chính xác đến từng mili giây
- 🔁 Ba chế độ chạy: đè để chạy, đè để lặp, bấm để chạy/dừng
- 🛟 Tự dừng và nhả mọi phím khi đổi cửa sổ hoặc mất kết nối chuột

### Ứng dụng cài đặt

- 🎨 Giao diện GTK 4 và libadwaita, không cần sửa file bằng tay
- 🌐 Có tiếng Anh, tiếng Việt và tiếng Trung giản thể, đổi được ngay trong app
- ⌨️ Ghi phím bằng cách nhấn thật trên bàn phím
- 💾 Lưu là có hiệu lực sau khoảng 1 giây, không cần nút Áp dụng
- 🧾 Giữ nguyên comment trong file cấu hình và không lưu giá trị sai
- 🔌 Bật/tắt chạy nền ngay trong app

### Ổn định và an toàn

- ♻️ Tự kết nối lại khi rút rồi cắm lại chuột hoặc receiver
- 🧯 Kernel tự trả chuột khi chương trình kết thúc, kể cả khi bị crash
- 🔒 Quy tắc udev chỉ cấp quyền cho đúng con chuột bạn chọn, không cho đọc bàn phím
- 🛑 Chuột trái bị khóa mặc định để bạn không mất điều khiển

## ⚙️ Cách hoạt động

```mermaid
flowchart LR
    A[Chuột thật] -->|evdev grab| B[clickshift]
    B -->|uinput| C[Chuột và bàn phím ảo]
    C --> D[GNOME và ứng dụng]
    E[GNOME Shell extension] -->|D-Bus: app đang focus| B
    F[clickshift-settings] -->|config.toml| B
```

## 🎬 Macro

Viết macro theo cú pháp quen thuộc của X-Mouse Button Control:

```toml
[apps.steam_proton]
side = "macro-hold:e{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}"
```

| Tiền tố | Cách hoạt động |
|---|---|
| `macro-hold:` | Chạy một lần khi đè, thả nút là dừng |
| `macro-repeat:` | Lặp liên tục khi đè, thả nút là dừng |
| `macro:` | Bấm để chạy, bấm lần nữa để dừng |

| Token | Ý nghĩa |
|---|---|
| `e`, `q`, `w` | Gõ phím thường, theo bố cục bàn phím US |
| `{LMB}` `{RMB}` `{MMB}` `{MB4}` `{MB5}` | Bấm chuột; thêm `D` hoặc `U` để nhấn hoặc nhả, như `{LMBD}` |
| `{WAITMS:160}` | Chờ 160 mili giây |
| `{MWUP}` `{MWDN}` `{MWLT}` `{MWRT}` | Cuộn một nấc |
| `{Enter}` `{F5}` `{Ctrl+C}` | Bấm phím hoặc tổ hợp phím |
| `{PRESS:Ctrl}` `{RELEASE:Ctrl}` | Nhấn giữ hoặc nhả một phím |

Token không có trong bảng sẽ bị từ chối kèm thông báo lỗi. Xem toàn bộ cú pháp cấu
hình trong [`config.example.toml`](config.example.toml).

> [!WARNING]
> Nhiều game online cấm macro tự động. Hãy kiểm tra điều khoản của game trước khi dùng.

## 📦 Cài đặt

Bạn cần Rust cùng thư viện phát triển của GTK 4 và libadwaita. Hãy chạy các lệnh
`sudo` trong terminal thật, vì chúng sẽ hỏi mật khẩu.

```sh
git clone https://github.com/1Tyson/clickshift.git
cd clickshift
cargo build --release --workspace
```

**1. Cấp quyền cho chuột.** Trước đó, hãy sửa tên chuột trong file quy tắc nếu bạn
dùng chuột khác. Dùng `target/release/clickshift list` để xem tên, hoặc `evtest` nếu
lệnh đó chưa thấy chuột.

```sh
sudo install -m 644 packaging/70-clickshift.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input --action=change
```

Nếu `clickshift list` vẫn không thấy chuột, hãy rút rồi cắm lại receiver.

**2. Tạo cấu hình.**

```sh
mkdir -p ~/.config/clickshift
cp config.example.toml ~/.config/clickshift/config.toml
```

**3. Cài GNOME extension.** Sau khi chép, hãy đăng xuất và đăng nhập lại rồi mới bật.

```sh
cp -r extension/clickshift@clickshift.github.io ~/.local/share/gnome-shell/extensions/
gnome-extensions enable clickshift@clickshift.github.io
```

**4. Cho chạy nền khi đăng nhập.**

```sh
install -D target/release/clickshift ~/.local/bin/clickshift
install -Dm 644 packaging/clickshift.service ~/.config/systemd/user/clickshift.service
systemctl --user daemon-reload
systemctl --user enable --now clickshift.service
```

**5. Thêm app cài đặt vào menu.**

```sh
install -D target/release/clickshift-settings ~/.local/bin/clickshift-settings
install -Dm 644 packaging/io.github.clickshift.Settings.desktop ~/.local/share/applications/io.github.clickshift.Settings.desktop
```

Đừng chạy clickshift cùng lúc với một preset của input-remapper trên cùng con chuột,
vì mỗi lúc chỉ một chương trình giữ được chuột.

## ▶️ Cách chạy

**Thử trước.** Lệnh đầu không thay đổi gì, lệnh sau tự trả chuột sau 30 giây. Nhấn
Ctrl+C trên bàn phím để dừng sớm.

```sh
target/release/clickshift run --dry-run --verbose
target/release/clickshift run --grab-seconds 30 --verbose
```

**Mở app cài đặt.** Sau bước 5 ở trên, nhấn phím Super rồi tìm **Clickshift**, hoặc chạy:

```sh
target/release/clickshift-settings
```

**Quản lý dịch vụ nền.**

```sh
systemctl --user status clickshift.service         # xem có đang chạy không
journalctl --user -u clickshift.service -f         # xem log
systemctl --user restart clickshift.service        # khởi động lại
systemctl --user disable --now clickshift.service  # dừng và tắt tự chạy
```

Sau khi build lại, chạy lại lệnh `install` của `clickshift` ở bước 4 rồi khởi động lại dịch vụ.

## 🛠️ Kiểm thử

```sh
cargo test    # chỉ kiểm tra logic, không cần quyền gì
```

Bộ test đầu cuối chạy binary thật với một chuột ảo. Nó chiếm các thiết bị đầu ra
trước khi gửi tín hiệu, nên không có gì lọt ra desktop, và không đụng tới chuột thật.
Bộ test này cần thêm một quy tắc udev:

```sh
sudo install -m 644 packaging/71-clickshift-test.rules /etc/udev/rules.d/
sudo udevadm control --reload
cargo test --test e2e -- --ignored
```

## ⚠️ Giới hạn

- Mới chỉ được thử trên CachyOS với GNOME Shell 51 và Wayland.
- Profile theo app chỉ có trên GNOME; `metadata.json` hiện chỉ khai báo bản 51.
- Trên môi trường khác, mọi app sẽ dùng chung profile mặc định.
- Profile đi theo cửa sổ đang focus, không phải cửa sổ dưới con trỏ.
- Mỗi file cấu hình chỉ dùng cho một con chuột.
- Thông báo dòng lệnh và lỗi cấu hình chỉ có tiếng Anh.
- Ký tự trong macro giả định bố cục bàn phím US.
- Macro dừng khi đổi cửa sổ, nên không thể dùng macro để tự chuyển cửa sổ.
- Con lăn không dùng được macro, nút dính, lớp giữ hoặc bấm nhanh/giữ lâu.
- App cài đặt không mở khóa chuột trái; muốn đổi phải đặt `allow_left_remap` bằng tay.

## 📄 Giấy phép

Phát hành theo giấy phép [MIT](LICENSE).

---

<a id="zh-cn"></a>

# 🇨🇳 简体中文

[English](#english) · [Tiếng Việt](#tieng-viet) · **简体中文**

clickshift 是一款类似 Windows 上 [X-Mouse Button Control](https://www.highrez.co.uk/downloads/XMouseButtonControl.htm)
的应用，使用 Rust 从零重写，可在 GNOME Wayland 上运行。它与 X-Mouse Button Control
没有共用代码，也没有任何关联。

## ✨ 功能

### 按键与滚轮

- 🎯 将鼠标按键改为键盘按键、组合键、媒体键或其他鼠标按键
- 🖲️ 反转滚轮方向，或让每格滚动触发一个按键
- 🚀 一键运行 shell 命令
- 🚫 禁用某个按键，或保持其原有功能

### 按应用配置

- 🪟 每个应用都有自己的配置，切换窗口时自动切换
- 🗂️ 配置中未设置的按键沿用通用设置

### 高级操作

- ⏱️ 短按和长按分别执行不同操作
- 🤝 同时按下两个按键触发单独的操作
- 📌 粘滞按键：单击一次按住，再单击一次松开，便于拖动
- 🧱 最多增加 9 个层，可按住使用或切换开关

### 定时宏

- 🎬 串联按键和点击，等待时间可精确到毫秒
- 🔁 三种模式：按住时播放、按住时循环、单击开始/停止
- 🛟 切换窗口或鼠标断开时自动停止，并松开所有按键

### 设置应用

- 🎨 基于 GTK 4 和 libadwaita 的界面，无需手动编辑配置文件
- 🌐 支持英语、越南语和简体中文，可在应用内切换
- ⌨️ 在键盘上直接按键即可录制
- 💾 更改约 1 秒后生效，无需点击应用按钮
- 🧾 保留配置文件中的注释，并拒绝无效的值
- 🔌 在应用内开启或关闭后台服务

### 稳定与安全

- ♻️ 鼠标或接收器重新连接后自动恢复
- 🧯 程序结束时（即使崩溃）内核也会自动归还鼠标
- 🔒 udev 规则只对您指定的鼠标授权，不会开放键盘
- 🛑 左键默认锁定，避免您失去对鼠标的控制

## ⚙️ 工作原理

```mermaid
flowchart LR
    A[真实鼠标] -->|evdev grab| B[clickshift]
    B -->|uinput| C[虚拟鼠标和键盘]
    C --> D[GNOME 和应用]
    E[GNOME Shell 扩展] -->|D-Bus: 当前焦点应用| B
    F[clickshift-settings] -->|config.toml| B
```

## 📦 安装

您需要 Rust 以及 GTK 4 和 libadwaita 的开发文件。在 Arch 或 CachyOS 上：

```sh
sudo pacman -S --needed rust gtk4 libadwaita
```

请在真实的终端中运行所有 `sudo` 命令，因为它们会询问密码。

**1. 构建。**

```sh
git clone https://github.com/1Tyson/clickshift.git
cd clickshift
cargo build --release --workspace
```

**2. 授予用户访问鼠标的权限。** 规则默认针对 `AJAZZ 2.4G 8K`，请先将
`packaging/70-clickshift.rules` 中的名称改为您的鼠标名称。`sudo evtest` 可列出输入设备的名称。

```sh
sudo install -m 644 packaging/70-clickshift.rules /etc/udev/rules.d/
sudo udevadm control --reload
sudo udevadm trigger --subsystem-match=input --action=change
target/release/clickshift list
```

最后一条命令应显示您的鼠标。如果没有，请拔下并重新插入接收器。

**3. 创建配置。** 将 `[device]` 下的 `name` 设为您的鼠标名称。

```sh
mkdir -p ~/.config/clickshift
cp config.example.toml ~/.config/clickshift/config.toml
```

**4. 安装 GNOME 扩展。** 它会告诉 clickshift 当前焦点所在的应用。复制后注销并重新登录，然后启用。

```sh
cp -r extension/clickshift@clickshift.github.io ~/.local/share/gnome-shell/extensions/
```

```sh
gnome-extensions enable clickshift@clickshift.github.io
```

## ▶️ 运行

**先试用。** 第一条命令不会改变任何内容，第二条命令会在 30 秒后归还鼠标。在键盘上按
Ctrl+C 可提前停止。

```sh
target/release/clickshift run --dry-run --verbose
target/release/clickshift run --grab-seconds 30 --verbose
```

**保持运行。** 以下命令会立即启动 clickshift，并在每次登录时自动启动。

```sh
install -D target/release/clickshift ~/.local/bin/clickshift
install -Dm 644 packaging/clickshift.service ~/.config/systemd/user/clickshift.service
systemctl --user daemon-reload
systemctl --user enable --now clickshift.service
```

**打开设置应用。** 直接启动：

```sh
target/release/clickshift-settings
```

如需从应用菜单打开，请安装一次，然后搜索 **Clickshift**：

```sh
install -D target/release/clickshift-settings ~/.local/bin/clickshift-settings
install -Dm 644 packaging/io.github.clickshift.Settings.desktop ~/.local/share/applications/io.github.clickshift.Settings.desktop
```

**管理服务。**

```sh
systemctl --user status clickshift.service         # 查看是否正在运行
journalctl --user -u clickshift.service -f         # 查看日志
systemctl --user restart clickshift.service        # 重新启动
systemctl --user disable --now clickshift.service  # 停止并关闭自动启动
```

重新构建后，请再次运行上面第一条 `install` 命令并重新启动服务。

请勿在同一只鼠标上同时运行 clickshift 和 input-remapper 预设，因为同一时间只有一个程序能占用鼠标。

## 🎬 宏

宏使用 X-Mouse Button Control 用户熟悉的写法：

```toml
[apps.steam_proton]
side = "macro-hold:e{WAITMS:300}{LMB}{WAITMS:150}{LMB}{WAITMS:250}{RMB}"
```

在设置应用中添加宏：

1. 在左侧选择一个应用，或点击 **+** 添加。
2. 将某个按键设为 **宏**，并选择播放方式。
3. 点击 **编辑宏**，粘贴您的宏，然后点击 **保存**。

| 前缀 | 行为 |
|---|---|
| `macro-hold:` | 按住时播放一次，松开即停止 |
| `macro-repeat:` | 按住时循环播放，松开即停止 |
| `macro:` | 单击开始，再次单击停止 |

| 标记 | 含义 |
|---|---|
| `e`, `q`, `w` | 输入按键，按美式键盘布局 |
| `{LMB}` `{RMB}` `{MMB}` `{MB4}` `{MB5}` | 单击；加 `D` 或 `U` 表示按下或松开，如 `{LMBD}` |
| `{WAITMS:160}` | 等待 160 毫秒 |
| `{MWUP}` `{MWDN}` `{MWLT}` `{MWRT}` | 滚动一格 |
| `{Enter}` `{F5}` `{Ctrl+C}` | 按一次按键或组合键 |
| `{PRESS:Ctrl}` `{RELEASE:Ctrl}` | 按下或松开一个按键 |

此处未列出的标记会被拒绝并显示错误。其他 X-Mouse Button Control 标记（如 `{WAIT:1}`、
`{WAITMS:10-20}` 和 `{CTRL}a`）暂不支持。完整配置语法请参阅
[`config.example.toml`](config.example.toml)。

> [!WARNING]
> 许多网络游戏禁止使用自动宏。使用前请查看游戏条款。

## 🧪 测试

```sh
cargo test    # 仅测试逻辑，无需任何权限
```

端到端测试会让真实程序连接虚拟鼠标运行。它会在发送输入前占用输出设备，因此不会有输入到达桌面，也不会碰到真实鼠标。它需要一条额外的 udev 规则：

```sh
sudo install -m 644 packaging/71-clickshift-test.rules /etc/udev/rules.d/
sudo udevadm control --reload
cargo test --test e2e -- --ignored
```

## ⚠️ 限制

- 仅在 CachyOS 的 GNOME Shell 51 和 Wayland 上测试过。
- 按应用配置需要 GNOME，且 `metadata.json` 只声明了 Shell 51。在其他环境中，所有应用都使用默认配置。
- 配置跟随焦点窗口，而不是鼠标指针下方的窗口。
- 每个配置文件只对应一只鼠标。
- 命令行和配置错误信息仅提供英文。
- 宏中的字符按美式键盘布局处理。
- 切换焦点时宏会停止，因此宏无法自行切换窗口。
- 滚轮方向不能使用宏、粘滞按键、按住层或短按/长按。
- 设置应用无法解锁左键；需要手动设置 `allow_left_remap`。

## 📄 许可证

[MIT](LICENSE)
