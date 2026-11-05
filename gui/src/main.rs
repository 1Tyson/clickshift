//! Settings app for clickshift. It only edits the config file: the daemon picks
//! up each save by itself, so changes apply without an Apply button.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use adw::prelude::*;
use clickshift::config::{self, MacroMode};
use clickshift::device::readable_mouse_names;
use clickshift::document::{Form, Saved, Slot, Store, key_token};
use clickshift::macros::Macro;
use gtk::{gdk, gio, glib};
use i18n::{Text, tr};

mod i18n;

const APP_ID: &str = "io.github.clickshift.Settings";
const SERVICE: &str = "clickshift.service";
const EXTENSION: &str = "clickshift@clickshift.github.io";
const MAX_LAYER: u8 = 9;
const NEW_HOLD_MS: i64 = 300;
const HOLD_MS_STEP: i64 = 50;
const HOLD_MS_MAX: i64 = 2000;
/// X11 and Wayland keycodes are evdev codes shifted by this much.
const XKB_KEYCODE_OFFSET: u32 = 8;
/// Left/right Ctrl, Shift, Alt and Super, which only count as part of a chord.
const MODIFIER_CODES: [u16; 8] = [29, 97, 42, 54, 56, 100, 125, 126];

const MAIN_BUTTONS: [(&str, Text); 5] = [
    ("left", ["Left button", "Chuột trái", "左键"]),
    ("right", ["Right button", "Chuột phải", "右键"]),
    ("middle", ["Middle button", "Nút giữa", "中键"]),
    (
        "side",
        ["Side button (side)", "Nút bên (side)", "侧键 (side)"],
    ),
    (
        "extra",
        ["Side button (extra)", "Nút bên (extra)", "侧键 (extra)"],
    ),
];
const OTHER_BUTTONS: [(&str, Text); 3] = [
    (
        "forward",
        ["Forward button", "Nút forward", "前进键 (forward)"],
    ),
    ("back", ["Back button", "Nút back", "后退键 (back)"]),
    ("task", ["Task button", "Nút task", "任务键 (task)"]),
];
const WHEEL_INPUTS: [(&str, Text); 4] = [
    ("wheel_up", ["Scroll up", "Cuộn lên", "向上滚动"]),
    ("wheel_down", ["Scroll down", "Cuộn xuống", "向下滚动"]),
    ("wheel_left", ["Scroll left", "Cuộn trái", "向左滚动"]),
    ("wheel_right", ["Scroll right", "Cuộn phải", "向右滚动"]),
];
const MOUSE_OUTPUTS: [(&str, Text); 5] = [
    ("mouse_left", ["Left button", "Chuột trái", "左键"]),
    ("mouse_right", ["Right button", "Chuột phải", "右键"]),
    ("mouse_middle", ["Middle button", "Nút giữa", "中键"]),
    ("mouse_back", ["Back", "Quay lại", "后退"]),
    ("mouse_forward", ["Forward", "Đi tới", "前进"]),
];
const MACRO_HELP: Text = [
    "Plain characters are typed as keys, assuming a US keyboard layout.\n\
{LMB} {RMB} {MMB} {MB4} {MB5}: click; add D or U to press or release, as in {LMBD}\n\
{MWUP} {MWDN} {MWLT} {MWRT}: scroll one notch\n\
{WAITMS:160}: wait, in milliseconds\n\
{Enter} {F5} {Ctrl+C}: tap a key or key combination\n\
{PRESS:Ctrl} {RELEASE:Ctrl}: press or release a key\n\n\
A macro stops by itself when you switch windows.",
    "Ký tự thường được gõ như phím, theo bố cục bàn phím US.\n\
{LMB} {RMB} {MMB} {MB4} {MB5}: bấm chuột; thêm D hoặc U để nhấn hoặc nhả, như {LMBD}\n\
{MWUP} {MWDN} {MWLT} {MWRT}: cuộn một nấc\n\
{WAITMS:160}: chờ, tính bằng mili giây\n\
{Enter} {F5} {Ctrl+C}: bấm phím hoặc tổ hợp phím\n\
{PRESS:Ctrl} {RELEASE:Ctrl}: nhấn hoặc nhả một phím\n\n\
Macro tự dừng khi bạn đổi cửa sổ.",
    "普通字符按美式键盘布局作为按键输入。\n\
{LMB} {RMB} {MMB} {MB4} {MB5}：单击；加 D 或 U 表示按下或松开，如 {LMBD}\n\
{MWUP} {MWDN} {MWLT} {MWRT}：滚动一格\n\
{WAITMS:160}：等待，单位为毫秒\n\
{Enter} {F5} {Ctrl+C}：按一次按键或组合键\n\
{PRESS:Ctrl} {RELEASE:Ctrl}：按下或松开一个按键\n\n\
切换窗口时宏会自动停止。",
];
const MACRO_MODES: [(MacroMode, Text); 3] = [
    (
        MacroMode::WhileHeld,
        ["Play while held", "Đè để chạy", "按住时播放"],
    ),
    (
        MacroMode::RepeatWhileHeld,
        ["Repeat while held", "Đè để lặp", "按住时循环"],
    ),
    (
        MacroMode::Toggle,
        ["Click to start/stop", "Bấm để chạy/dừng", "单击开始/停止"],
    ),
];
const SCROLL_OUTPUTS: [(&str, Text); 4] = [
    ("up", ["Up", "Lên", "上"]),
    ("down", ["Down", "Xuống", "下"]),
    ("left", ["Left", "Trái", "左"]),
    ("right", ["Right", "Phải", "右"]),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Unset,
    Passthrough,
    Disabled,
    Keys,
    MouseButton,
    Scroll,
    Run,
    Sticky,
    Macro,
    LayerHold,
    LayerToggle,
    TapHold,
}

const BUTTON_KINDS: [Kind; 12] = [
    Kind::Unset,
    Kind::Passthrough,
    Kind::Disabled,
    Kind::Keys,
    Kind::MouseButton,
    Kind::Scroll,
    Kind::Run,
    Kind::Sticky,
    Kind::Macro,
    Kind::LayerHold,
    Kind::LayerToggle,
    Kind::TapHold,
];
/// A wheel notch has no release, so nothing that needs one is offered.
const WHEEL_KINDS: [Kind; 7] = [
    Kind::Unset,
    Kind::Passthrough,
    Kind::Disabled,
    Kind::Keys,
    Kind::Scroll,
    Kind::Run,
    Kind::LayerToggle,
];
const COMBO_KINDS: [Kind; 9] = [
    Kind::Disabled,
    Kind::Keys,
    Kind::MouseButton,
    Kind::Scroll,
    Kind::Run,
    Kind::Sticky,
    Kind::Macro,
    Kind::LayerHold,
    Kind::LayerToggle,
];

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Unset => tr(["Not set", "Không đặt", "未设置"]),
            Self::Passthrough => tr(["Unchanged", "Giữ nguyên", "保持不变"]),
            Self::Disabled => tr(["Disabled", "Tắt", "禁用"]),
            Self::Keys => tr(["Key / combination", "Phím / tổ hợp phím", "按键 / 组合键"]),
            Self::MouseButton => tr(["Other mouse button", "Nút chuột khác", "其他鼠标按键"]),
            Self::Scroll => tr(["Scroll", "Cuộn", "滚动"]),
            Self::Run => tr(["Run command", "Chạy lệnh", "运行命令"]),
            Self::Sticky => tr(["Sticky button", "Nút dính", "粘滞按键"]),
            Self::Macro => tr(["Macro", "Macro", "宏"]),
            Self::LayerHold => tr(["Hold for layer", "Giữ để dùng lớp", "按住使用层"]),
            Self::LayerToggle => tr(["Toggle layer", "Bật/tắt lớp", "切换层"]),
            Self::TapHold => tr(["Tap / hold", "Bấm nhanh / giữ lâu", "短按 / 长按"]),
        }
    }

    fn of(form: &Form) -> Self {
        match form {
            Form::Unset => Self::Unset,
            Form::Passthrough => Self::Passthrough,
            Form::Disabled => Self::Disabled,
            Form::Keys(_) => Self::Keys,
            Form::MouseButton(_) => Self::MouseButton,
            Form::Run(_) => Self::Run,
            Form::Sticky(_) => Self::Sticky,
            Form::Scroll(_) => Self::Scroll,
            Form::Macro(..) => Self::Macro,
            Form::LayerHold(_) => Self::LayerHold,
            Form::LayerToggle(_) => Self::LayerToggle,
            Form::TapHold { .. } => Self::TapHold,
        }
    }

    /// The form to show after choosing this kind. Kinds with a usable default
    /// are saved right away; the others wait until the user types a value.
    fn starting_form(self) -> (Form, bool) {
        match self {
            Self::Unset => (Form::Unset, true),
            Self::Passthrough => (Form::Passthrough, true),
            Self::Disabled => (Form::Disabled, true),
            Self::MouseButton => (Form::MouseButton("mouse_back".into()), true),
            Self::Scroll => (Form::Scroll("down".into()), true),
            Self::Sticky => (Form::Sticky("mouse_left".into()), true),
            Self::LayerHold => (Form::LayerHold(1), true),
            Self::LayerToggle => (Form::LayerToggle(1), true),
            Self::Keys => (Form::Keys(String::new()), false),
            Self::Run => (Form::Run(String::new()), false),
            Self::Macro => (Form::Macro(String::new(), MacroMode::WhileHeld), false),
            Self::TapHold => (
                Form::TapHold {
                    tap: String::new(),
                    hold: String::new(),
                    hold_ms: NEW_HOLD_MS,
                },
                false,
            ),
        }
    }
}

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    store: RefCell<Store>,
    layer: Cell<u8>,
    app: RefCell<Option<String>>,
    /// The app shown on each sidebar row; `None` is the all-apps row.
    sidebar_apps: RefCell<Vec<Option<String>>>,
    sidebar: gtk::ListBox,
    content: adw::Bin,
    remove_app: gtk::Button,
    refreshing_sidebar: Cell<bool>,
}

impl Ui {
    fn slot(&self) -> Slot {
        Slot {
            layer: self.layer.get(),
            app: self.app.borrow().clone(),
        }
    }

    fn toast(&self, message: &str) {
        self.toasts.add_toast(adw::Toast::new(message));
    }

    /// Returns true if the edit reached the file.
    fn save(self: &Rc<Self>, edit: impl FnOnce(&mut clickshift::document::Document)) -> bool {
        let result = self.store.borrow_mut().update(edit);
        match result {
            Ok(Saved::Written) => true,
            Ok(Saved::ReloadedFromDisk) => {
                self.toast(
                    tr(["The config file was changed elsewhere and has been reloaded. Please try again.", "File cấu hình vừa được sửa ở nơi khác nên đã tải lại. Hãy thao tác lại.", "配置文件已在其他地方被修改并已重新加载，请重试。"]),
                );
                self.refresh_later();
                false
            }
            Err(error) => {
                self.toast(&format!(
                    "{}: {error:#}",
                    tr(["Not saved", "Chưa lưu", "未保存"])
                ));
                false
            }
        }
    }

    fn commit(self: &Rc<Self>, input: &str, form: &Form, widget: Option<&gtk::Widget>) -> bool {
        let slot = self.slot();
        let saved = self.save(|doc| doc.set_form(&slot, input, form));
        if let Some(widget) = widget {
            if saved {
                widget.remove_css_class("error");
            } else {
                widget.add_css_class("error");
            }
        }
        saved
    }

    /// Rebuilding destroys the widget whose signal is running, so it waits.
    fn refresh_later(self: &Rc<Self>) {
        let ui = self.clone();
        glib::idle_add_local_once(move || ui.refresh());
    }

    fn refresh(self: &Rc<Self>) {
        self.refresh_sidebar();
        self.refresh_content();
    }

    fn refresh_sidebar(self: &Rc<Self>) {
        self.refreshing_sidebar.set(true);
        while let Some(child) = self.sidebar.first_child() {
            self.sidebar.remove(&child);
        }

        let mut entries: Vec<Option<String>> = vec![None];
        entries.extend(self.store.borrow().doc().apps().into_iter().map(Some));
        let current = self.app.borrow().clone();
        if !entries.contains(&current) {
            *self.app.borrow_mut() = None;
        }

        let selected = self.app.borrow().clone();
        for entry in &entries {
            let text = entry
                .as_deref()
                .unwrap_or(tr(["All apps", "Mọi ứng dụng", "所有应用"]));
            let label = gtk::Label::builder()
                .label(text)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .build();
            let row = gtk::ListBoxRow::builder().child(&label).build();
            self.sidebar.append(&row);
            if *entry == selected {
                self.sidebar.select_row(Some(&row));
            }
        }
        *self.sidebar_apps.borrow_mut() = entries;
        self.remove_app.set_sensitive(selected.is_some());
        self.refreshing_sidebar.set(false);
    }

    fn refresh_content(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        page.add(&self.device_group());
        page.add(&self.buttons_group());
        page.add(&self.wheel_group());
        page.add(&self.combos_group());
        self.content.set_child(Some(&page));
    }

    fn device_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr(["Mouse", "Chuột", "鼠标"]))
            .build();
        let current = self.store.borrow().doc().device_name();
        let mut names = readable_mouse_names();
        if !names.contains(&current) {
            names.insert(0, current.clone());
        }
        let labels: Vec<&str> = names.iter().map(String::as_str).collect();
        let row = adw::ComboRow::builder()
            .title(tr(["Device", "Thiết bị", "设备"]))
            .subtitle(tr([
                "Shows only mice this account can read under the udev rule",
                "Chỉ hiện chuột mà tài khoản này đọc được theo quy tắc udev",
                "仅显示此账户可根据 udev 规则读取的鼠标",
            ]))
            .model(&gtk::StringList::new(&labels))
            .build();
        if let Some(index) = names.iter().position(|name| *name == current) {
            row.set_selected(index as u32);
        }
        let ui = self.clone();
        row.connect_selected_notify(move |row| {
            if let Some(name) = names.get(row.selected() as usize) {
                ui.save(|doc| doc.set_device_name(name));
            }
        });
        group.add(&row);
        group
    }

    fn buttons_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr(["Mouse buttons", "Nút chuột", "鼠标按键"]))
            .build();
        let allow_left = self.store.borrow().doc().allow_left_remap();
        for (input, title) in MAIN_BUTTONS {
            let row = self.action_row(input, tr(title), &BUTTON_KINDS);
            if input == "left" && !allow_left {
                row.set_sensitive(false);
                row.set_subtitle(tr([
                    "Locked so you cannot lose control of the mouse",
                    "Bị khóa để tránh mất điều khiển chuột",
                    "已锁定，以免失去对鼠标的控制",
                ]));
            }
            group.add(&row);
        }
        let others = adw::ExpanderRow::builder()
            .title(tr(["Other buttons", "Nút khác", "其他按键"]))
            .subtitle(tr([
                "Only useful if your mouse has these buttons",
                "Chỉ dùng nếu chuột của bạn có các nút này",
                "仅当您的鼠标有这些按键时才有用",
            ]))
            .build();
        for (input, title) in OTHER_BUTTONS {
            others.add_row(&self.action_row(input, tr(title), &BUTTON_KINDS));
        }
        group.add(&others);
        group
    }

    fn wheel_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr(["Wheel", "Con lăn", "滚轮"]))
            .build();
        for (input, title) in WHEEL_INPUTS {
            group.add(&self.action_row(input, tr(title), &WHEEL_KINDS));
        }
        group
    }

    fn combos_group(self: &Rc<Self>) -> adw::PreferencesGroup {
        let group = adw::PreferencesGroup::builder()
            .title(tr(["Button combos", "Tổ hợp nút chuột", "鼠标组合键"]))
            .description(tr(["Press two buttons at almost the same time. Buttons in a combo respond slightly later.", "Bấm hai nút gần như cùng lúc. Nút trong tổ hợp sẽ phản hồi trễ một chút.", "几乎同时按下两个按键。组合中的按键响应会稍有延迟。"]))
            .build();

        let combos = self.store.borrow().doc().combos(&self.slot());
        for combo in combos {
            let title = combo
                .split('+')
                .map(button_title)
                .collect::<Vec<_>>()
                .join(" + ");
            let row = self.action_row(&combo, &title, &COMBO_KINDS);
            let delete = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr(["Remove combo", "Xóa tổ hợp", "删除组合"]))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let ui = self.clone();
            delete.connect_clicked(move |_| {
                if ui.commit(&combo, &Form::Unset, None) {
                    ui.refresh_later();
                }
            });
            row.add_suffix(&delete);
            group.add(&row);
        }

        group.add(&self.new_combo_row());
        group
    }

    fn new_combo_row(self: &Rc<Self>) -> adw::ActionRow {
        let allow_left = self.store.borrow().doc().allow_left_remap();
        let choices: Vec<(&str, Text)> = MAIN_BUTTONS
            .into_iter()
            .chain(OTHER_BUTTONS)
            .filter(|(input, _)| allow_left || *input != "left")
            .collect();
        let labels: Vec<&str> = choices.iter().map(|(_, title)| tr(*title)).collect();

        let first = gtk::DropDown::from_strings(&labels);
        let second = gtk::DropDown::from_strings(&labels);
        second.set_selected(1);
        first.set_valign(gtk::Align::Center);
        second.set_valign(gtk::Align::Center);
        let add = gtk::Button::builder()
            .label(tr(["Add", "Thêm", "添加"]))
            .valign(gtk::Align::Center)
            .build();

        let row = adw::ActionRow::builder()
            .title(tr(["Add combo", "Thêm tổ hợp", "添加组合"]))
            .build();
        row.add_suffix(&first);
        row.add_suffix(&second);
        row.add_suffix(&add);

        let ui = self.clone();
        add.connect_clicked(move |_| {
            let (a, _) = choices[first.selected() as usize];
            let (b, _) = choices[second.selected() as usize];
            // Starts disabled, so the combo exists and can be given an action.
            if ui.commit(&format!("{a}+{b}"), &Form::Disabled, None) {
                ui.refresh_later();
            }
        });
        row
    }

    fn action_row(self: &Rc<Self>, input: &str, title: &str, kinds: &[Kind]) -> adw::ActionRow {
        let form = self.store.borrow().doc().form(&self.slot(), input);
        let current = Kind::of(&form);

        // A value written by hand may not be offered for this input; keep it
        // visible rather than silently showing something else.
        let mut kinds = kinds.to_vec();
        if !kinds.contains(&current) {
            kinds.push(current);
        }

        let labels: Vec<&str> = kinds.iter().map(|kind| kind.label()).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        dropdown.set_valign(gtk::Align::Center);
        if let Some(index) = kinds.iter().position(|kind| *kind == current) {
            dropdown.set_selected(index as u32);
        }

        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        self.fill_editor(&editor, input, &form);

        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&editor);
        row.add_suffix(&dropdown);

        let ui = self.clone();
        let input = input.to_owned();
        dropdown.connect_selected_notify(move |dropdown| {
            let (form, complete) = kinds[dropdown.selected() as usize].starting_form();
            if complete {
                ui.commit(&input, &form, None);
            }
            ui.fill_editor(&editor, &input, &form);
        });
        row
    }

    fn reopen(self: &Rc<Self>) {
        let path = self.store.borrow().path().to_owned();
        let app = self.window.application().and_downcast::<adw::Application>();
        let (Some(app), Ok(store)) = (app, open_store(&path)) else {
            self.toast(tr([
                "Cannot read the config file",
                "Không đọc được file cấu hình",
                "无法读取配置文件",
            ]));
            return;
        };
        build_window(&app, store).present();
        self.window.close();
    }

    fn fill_editor(self: &Rc<Self>, editor: &gtk::Box, input: &str, form: &Form) {
        while let Some(child) = editor.first_child() {
            editor.remove(&child);
        }
        match form {
            Form::Unset | Form::Passthrough | Form::Disabled => {}
            Form::Keys(text) => {
                self.keys_editor(editor, input, text, "Ctrl+W", Form::Keys);
            }
            Form::Sticky(text) => {
                self.keys_editor(editor, input, text, "mouse_left", Form::Sticky);
            }
            Form::Run(text) => {
                let entry = text_entry(text, "gnome-calculator", 22);
                let ui = self.clone();
                let input = input.to_owned();
                wire_entry(
                    &entry,
                    Rc::new(move |entry: &gtk::Entry| {
                        let form = Form::Run(entry.text().trim().to_owned());
                        ui.commit(&input, &form, Some(entry.upcast_ref()));
                    }),
                );
                editor.append(&entry);
            }
            Form::Macro(text, mode) => self.macro_editor(editor, input, text, *mode),
            Form::MouseButton(current) => {
                self.choice_editor(editor, input, &MOUSE_OUTPUTS, current, |value| {
                    Form::MouseButton(value.to_owned())
                });
            }
            Form::Scroll(current) => {
                self.choice_editor(editor, input, &SCROLL_OUTPUTS, current, |value| {
                    Form::Scroll(value.to_owned())
                });
            }
            Form::LayerHold(layer) => {
                self.layer_editor(editor, input, *layer, Form::LayerHold);
            }
            Form::LayerToggle(layer) => {
                self.layer_editor(editor, input, *layer, Form::LayerToggle);
            }
            Form::TapHold { tap, hold, hold_ms } => {
                self.tap_hold_editor(editor, input, tap, hold, *hold_ms);
            }
        }
    }

    fn keys_editor(
        self: &Rc<Self>,
        editor: &gtk::Box,
        input: &str,
        text: &str,
        placeholder: &str,
        make_form: impl Fn(String) -> Form + 'static,
    ) {
        let entry = text_entry(text, placeholder, 14);
        let ui = self.clone();
        let input = input.to_owned();
        let commit = wire_entry(
            &entry,
            Rc::new(move |entry: &gtk::Entry| {
                let form = make_form(entry.text().trim().to_owned());
                ui.commit(&input, &form, Some(entry.upcast_ref()));
            }),
        );
        editor.append(&entry);
        editor.append(&self.capture_button(&entry, commit));
    }

    /// A macro is too long to read in a row, so it gets its own dialog.
    fn macro_editor(self: &Rc<Self>, editor: &gtk::Box, input: &str, text: &str, mode: MacroMode) {
        let summary = gtk::Label::new(Some(&macro_summary(text)));
        summary.add_css_class("dim-label");
        let open = gtk::Button::with_label(tr(["Edit macro", "Sửa macro", "编辑宏"]));
        let current = Rc::new(RefCell::new(text.to_owned()));
        let mode = Rc::new(Cell::new(mode));

        let labels: Vec<&str> = MACRO_MODES.iter().map(|(_, label)| tr(*label)).collect();
        let modes = gtk::DropDown::from_strings(&labels);
        if let Some(index) = MACRO_MODES.iter().position(|(m, _)| *m == mode.get()) {
            modes.set_selected(index as u32);
        }
        {
            let ui = self.clone();
            let input = input.to_owned();
            let (current, mode) = (current.clone(), mode.clone());
            modes.connect_selected_notify(move |modes| {
                let (chosen, _) = MACRO_MODES[modes.selected() as usize];
                mode.set(chosen);
                // With no macro yet, the choice is saved along with the text.
                let text = current.borrow().clone();
                if !text.is_empty() {
                    ui.commit(&input, &Form::Macro(text, chosen), None);
                }
            });
        }
        {
            let ui = self.clone();
            let input = input.to_owned();
            let summary = summary.clone();
            open.connect_clicked(move |_| {
                ui.show_macro_dialog(&input, &current, &mode, &summary);
            });
        }
        editor.append(&summary);
        editor.append(&open);
        editor.append(&modes);
    }

    fn show_macro_dialog(
        self: &Rc<Self>,
        input: &str,
        current: &Rc<RefCell<String>>,
        mode: &Rc<Cell<MacroMode>>,
        summary: &gtk::Label,
    ) {
        let view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .monospace(true)
            .top_margin(8)
            .bottom_margin(8)
            .left_margin(8)
            .right_margin(8)
            .build();
        view.buffer().set_text(&current.borrow());
        let scroller = gtk::ScrolledWindow::builder()
            .child(&view)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .css_classes(["card"])
            .build();
        let status = gtk::Label::builder().xalign(0.0).wrap(true).build();
        let help = gtk::Label::builder()
            .label(tr(MACRO_HELP))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["dim-label", "caption"])
            .build();

        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        body.append(&scroller);
        body.append(&status);
        body.append(&help);

        let save = gtk::Button::builder()
            .label(tr(["Save", "Lưu", "保存"]))
            .css_classes(["suggested-action"])
            .build();
        let header = adw::HeaderBar::new();
        header.pack_end(&save);
        let frame = adw::ToolbarView::new();
        frame.add_top_bar(&header);
        frame.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(tr(["Edit macro", "Sửa macro", "编辑宏"]))
            .content_width(620)
            .content_height(520)
            .child(&frame)
            .build();

        let show_status = {
            let (status, save) = (status.clone(), save.clone());
            move |buffer: &gtk::TextBuffer| match Macro::parse(&macro_text(buffer)) {
                Ok(parsed) => {
                    status.remove_css_class("error");
                    status.set_text(&format!(
                        "{} {}",
                        tr(["Valid, length:", "Hợp lệ, dài:", "有效，时长："]),
                        seconds(parsed.total_ms)
                    ));
                    save.set_sensitive(true);
                }
                Err(error) => {
                    status.add_css_class("error");
                    status.set_text(&format!("{error:#}"));
                    save.set_sensitive(false);
                }
            }
        };
        show_status(&view.buffer());
        view.buffer().connect_changed(show_status);

        let ui = self.clone();
        let input = input.to_owned();
        let (current, mode) = (current.clone(), mode.clone());
        let (summary, closing) = (summary.clone(), dialog.clone());
        save.connect_clicked(move |_| {
            let text = macro_text(&view.buffer());
            if ui.commit(&input, &Form::Macro(text.clone(), mode.get()), None) {
                summary.set_text(&macro_summary(&text));
                *current.borrow_mut() = text;
                closing.close();
            }
        });
        dialog.present(Some(&self.window));
    }

    fn capture_button(self: &Rc<Self>, entry: &gtk::Entry, commit: Rc<dyn Fn()>) -> gtk::Button {
        let button = gtk::Button::builder()
            .icon_name("input-keyboard-symbolic")
            .tooltip_text(tr([
                "Record a key by pressing it on the keyboard",
                "Ghi phím bằng cách nhấn trên bàn phím",
                "在键盘上按键以录制",
            ]))
            .build();
        let ui = self.clone();
        let entry = entry.clone();
        button.connect_clicked(move |_| {
            let entry = entry.clone();
            let commit = commit.clone();
            ui.capture_keys(move |chord| {
                entry.set_text(&chord);
                commit();
            });
        });
        button
    }

    fn capture_keys(self: &Rc<Self>, on_chord: impl Fn(String) + 'static) {
        let label = gtk::Label::builder()
            .label(tr([
                "Press the key or combination to assign.\nPress Esc to cancel.",
                "Nhấn phím hoặc tổ hợp phím cần gán.\nNhấn Esc để hủy.",
                "请按下要分配的按键或组合键。\n按 Esc 取消。",
            ]))
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .margin_bottom(24)
            .margin_start(24)
            .margin_end(24)
            .build();
        let dialog = adw::Dialog::builder()
            .title(tr(["Record key", "Ghi phím", "录制按键"]))
            .child(&label)
            .build();

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let closing = dialog.clone();
        keys.connect_key_pressed(move |_, keyval, keycode, state| {
            if keyval == gdk::Key::Escape && !has_modifier(state) {
                closing.close();
                return glib::Propagation::Stop;
            }
            if let Some(chord) = chord_from_event(keycode, state) {
                on_chord(chord);
                closing.close();
            }
            glib::Propagation::Stop
        });
        dialog.add_controller(keys);
        dialog.present(Some(&self.window));
    }

    fn choice_editor(
        self: &Rc<Self>,
        editor: &gtk::Box,
        input: &str,
        choices: &'static [(&'static str, Text)],
        current: &str,
        make_form: impl Fn(&str) -> Form + 'static,
    ) {
        let labels: Vec<&str> = choices.iter().map(|(_, label)| tr(*label)).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        if let Some(index) = choices.iter().position(|(value, _)| *value == current) {
            dropdown.set_selected(index as u32);
        }
        let ui = self.clone();
        let input = input.to_owned();
        dropdown.connect_selected_notify(move |dropdown| {
            let (value, _) = choices[dropdown.selected() as usize];
            ui.commit(&input, &make_form(value), None);
        });
        editor.append(&dropdown);
    }

    /// Dropdowns rather than spin buttons here and below: a spin button changes
    /// its value when the wheel passes over it while scrolling the page.
    fn layer_editor(
        self: &Rc<Self>,
        editor: &gtk::Box,
        input: &str,
        layer: u8,
        make_form: fn(u8) -> Form,
    ) {
        let names: Vec<String> = (1..=MAX_LAYER).map(|layer| layer.to_string()).collect();
        let labels: Vec<&str> = names.iter().map(String::as_str).collect();
        let dropdown = gtk::DropDown::from_strings(&labels);
        dropdown.set_selected(u32::from(layer.saturating_sub(1)));
        let ui = self.clone();
        let input = input.to_owned();
        dropdown.connect_selected_notify(move |dropdown| {
            let form = make_form(dropdown.selected() as u8 + 1);
            ui.commit(&input, &form, None);
        });
        editor.append(&dropdown);
    }

    fn tap_hold_editor(
        self: &Rc<Self>,
        editor: &gtk::Box,
        input: &str,
        tap: &str,
        hold: &str,
        hold_ms: i64,
    ) {
        let tap_entry = text_entry(tap, "Alt+Left", 10);
        let hold_entry = text_entry(hold, "Ctrl+W", 10);
        // Keeps a value written by hand, even if it is not one of the usual steps.
        let mut hold_times: Vec<i64> = (HOLD_MS_STEP..=HOLD_MS_MAX)
            .step_by(HOLD_MS_STEP as usize)
            .collect();
        if !hold_times.contains(&hold_ms) {
            hold_times.push(hold_ms);
            hold_times.sort_unstable();
        }
        let names: Vec<String> = hold_times.iter().map(|ms| format!("{ms} ms")).collect();
        let labels: Vec<&str> = names.iter().map(String::as_str).collect();
        let milliseconds = gtk::DropDown::from_strings(&labels);
        if let Some(index) = hold_times.iter().position(|ms| *ms == hold_ms) {
            milliseconds.set_selected(index as u32);
        }
        milliseconds.set_tooltip_text(Some(tr([
            "How long a press counts as a hold",
            "Giữ bao lâu thì tính là giữ lâu",
            "按住多久算作长按",
        ])));

        let commit_all: Rc<dyn Fn()> = {
            let ui = self.clone();
            let input = input.to_owned();
            let (tap_entry, hold_entry, milliseconds) =
                (tap_entry.clone(), hold_entry.clone(), milliseconds.clone());
            Rc::new(move || {
                let tap = tap_entry.text().trim().to_owned();
                let hold = hold_entry.text().trim().to_owned();
                if tap.is_empty() || hold.is_empty() {
                    return;
                }
                let form = Form::TapHold {
                    tap,
                    hold,
                    hold_ms: hold_times[milliseconds.selected() as usize],
                };
                let saved = ui.commit(&input, &form, None);
                for entry in [&tap_entry, &hold_entry] {
                    if saved {
                        entry.remove_css_class("error");
                    } else {
                        entry.add_css_class("error");
                    }
                }
            })
        };

        let grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(6)
            .margin_top(6)
            .margin_bottom(6)
            .build();
        for (row, (title, entry)) in [
            (tr(["Tap", "Nhanh", "短按"]), &tap_entry),
            (tr(["Hold", "Giữ", "长按"]), &hold_entry),
        ]
        .into_iter()
        .enumerate()
        {
            let commit = commit_all.clone();
            let wired = wire_entry(entry, Rc::new(move |_: &gtk::Entry| commit()));
            let label = gtk::Label::builder().label(title).xalign(0.0).build();
            grid.attach(&label, 0, row as i32, 1, 1);
            grid.attach(entry, 1, row as i32, 1, 1);
            grid.attach(&self.capture_button(entry, wired), 2, row as i32, 1, 1);
        }
        let commit = commit_all;
        milliseconds.connect_selected_notify(move |_| commit());
        let label = gtk::Label::builder()
            .label(tr(["Time", "Thời gian", "时间"]))
            .xalign(0.0)
            .build();
        grid.attach(&label, 0, 2, 1, 1);
        grid.attach(&milliseconds, 1, 2, 2, 1);
        editor.append(&grid);
    }

    fn show_app_picker(self: &Rc<Self>) {
        let mut apps: Vec<gio::AppInfo> = gio::AppInfo::all()
            .into_iter()
            .filter(|app| app.should_show())
            .collect();
        apps.sort_by_key(|app| app.display_name().to_lowercase());
        let apps = Rc::new(apps);

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr([
                "Search for an app, or type a name and press Enter",
                "Tìm ứng dụng, hoặc nhập tên rồi nhấn Enter",
                "搜索应用，或输入名称后按 Enter",
            ]))
            .build();
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        for app in apps.iter() {
            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&app.display_name()).as_str())
                .subtitle(glib::markup_escape_text(&app_name_of(app)).as_str())
                .activatable(true)
                .build();
            if let Some(icon) = app.icon() {
                let image = gtk::Image::from_gicon(&icon);
                image.set_pixel_size(32);
                row.add_prefix(&image);
            }
            list.append(&row);
        }

        {
            let apps = apps.clone();
            let search = search.clone();
            list.set_filter_func(move |row| {
                let needle = search.text().to_lowercase();
                apps.get(row.index() as usize).is_some_and(|app| {
                    needle.is_empty()
                        || app.display_name().to_lowercase().contains(&needle)
                        || app_name_of(app).to_lowercase().contains(&needle)
                })
            });
        }
        {
            let list = list.clone();
            search.connect_search_changed(move |_| list.invalidate_filter());
        }

        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        body.append(&search);
        body.append(&scroller);

        let frame = adw::ToolbarView::new();
        frame.add_top_bar(&adw::HeaderBar::new());
        frame.set_content(Some(&body));
        let dialog = adw::Dialog::builder()
            .title(tr(["Add app", "Thêm ứng dụng", "添加应用"]))
            .content_width(460)
            .content_height(560)
            .child(&frame)
            .build();

        {
            let ui = self.clone();
            let dialog = dialog.clone();
            list.connect_row_activated(move |_, row| {
                if let Some(app) = apps.get(row.index() as usize) {
                    ui.add_app(&app_name_of(app));
                    dialog.close();
                }
            });
        }
        {
            let ui = self.clone();
            let dialog = dialog.clone();
            search.connect_activate(move |search| {
                let name = search.text().trim().to_owned();
                if !name.is_empty() {
                    ui.add_app(&name);
                    dialog.close();
                }
            });
        }
        dialog.present(Some(&self.window));
    }

    fn add_app(self: &Rc<Self>, name: &str) {
        if self.save(|doc| doc.add_app(name)) {
            *self.app.borrow_mut() = Some(name.to_owned());
            self.refresh_later();
        }
    }

    fn confirm_remove_app(self: &Rc<Self>) {
        let Some(app) = self.app.borrow().clone() else {
            return;
        };
        let dialog = adw::AlertDialog::new(
            Some(tr(["Remove this app profile?", "Xóa profile của ứng dụng?", "删除此应用的配置?"])),
            Some(&tr(["All settings for \"{app}\" on every layer will be removed from the config file.", "Mọi thiết lập của \"{app}\" trên tất cả các lớp sẽ bị xóa khỏi file cấu hình.", "“{app}”在所有层上的设置都将从配置文件中删除。"]).replace("{app}", &app)),
        );
        dialog.add_responses(&[
            ("cancel", tr(["Cancel", "Hủy", "取消"])),
            ("remove", tr(["Remove", "Xóa", "删除"])),
        ]);
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("cancel"));
        let ui = self.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "remove" && ui.save(|doc| doc.remove_app(&app)) {
                *ui.app.borrow_mut() = None;
                ui.refresh_later();
            }
        });
        dialog.present(Some(&self.window));
    }
}

/// Line breaks only help reading in the editor; they are never part of a macro.
fn macro_text(buffer: &gtk::TextBuffer) -> String {
    let (start, end) = buffer.bounds();
    let text = buffer.text(&start, &end, false);
    text.lines().map(str::trim).collect()
}

fn seconds(milliseconds: u64) -> String {
    format!(
        "{:.1} {}",
        milliseconds as f64 / 1000.0,
        tr(["s", "giây", "秒"])
    )
}

fn macro_summary(text: &str) -> String {
    match Macro::parse(text) {
        Ok(parsed) => seconds(parsed.total_ms),
        Err(_) => tr(["No macro yet", "Chưa có macro", "尚无宏"]).to_owned(),
    }
}

fn button_title(input: &str) -> String {
    MAIN_BUTTONS
        .into_iter()
        .chain(OTHER_BUTTONS)
        .find(|(name, _)| *name == input.trim().to_lowercase())
        .map_or_else(
            || input.trim().to_owned(),
            |(_, title)| tr(title).to_owned(),
        )
}

/// The name a config uses for an app: its desktop id without the suffix.
fn app_name_of(app: &gio::AppInfo) -> String {
    let id = app.id().map(|id| id.to_string()).unwrap_or_default();
    id.strip_suffix(".desktop").unwrap_or(&id).to_owned()
}

fn has_modifier(state: gdk::ModifierType) -> bool {
    state.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::SHIFT_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK,
    )
}

/// `None` while only modifiers are down, since the chord is not finished yet.
fn chord_from_event(keycode: u32, state: gdk::ModifierType) -> Option<String> {
    let code = u16::try_from(keycode.checked_sub(XKB_KEYCODE_OFFSET)?).ok()?;
    if MODIFIER_CODES.contains(&code) {
        return None;
    }
    let key = key_token(code)?;
    let mut parts = Vec::new();
    for (mask, name) in [
        (gdk::ModifierType::CONTROL_MASK, "Ctrl"),
        (gdk::ModifierType::SHIFT_MASK, "Shift"),
        (gdk::ModifierType::ALT_MASK, "Alt"),
        (gdk::ModifierType::SUPER_MASK, "Super"),
    ] {
        if state.contains(mask) {
            parts.push(name.to_owned());
        }
    }
    parts.push(key);
    Some(parts.join("+"))
}

fn text_entry(text: &str, placeholder: &str, width: i32) -> gtk::Entry {
    gtk::Entry::builder()
        .text(text)
        .placeholder_text(placeholder)
        .width_chars(width)
        .valign(gtk::Align::Center)
        .build()
}

/// Commits on Enter and when focus leaves, skipping empty or unchanged text.
/// The returned closure commits on demand, for the key capture button.
fn wire_entry(entry: &gtk::Entry, on_commit: Rc<dyn Fn(&gtk::Entry)>) -> Rc<dyn Fn()> {
    let last = Rc::new(RefCell::new(entry.text().to_string()));
    let commit: Rc<dyn Fn()> = {
        let entry = entry.clone();
        Rc::new(move || {
            let text = entry.text().to_string();
            if text.trim().is_empty() || *last.borrow() == text {
                return;
            }
            *last.borrow_mut() = text;
            on_commit(&entry);
        })
    };
    {
        let commit = commit.clone();
        entry.connect_activate(move |_| commit());
    }
    let focus = gtk::EventControllerFocus::new();
    {
        let commit = commit.clone();
        focus.connect_leave(move |_| commit());
    }
    entry.add_controller(focus);
    commit
}

fn systemctl(args: &[&str]) -> Option<std::process::Output> {
    Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .ok()
}

fn service_installed() -> bool {
    systemctl(&["cat", SERVICE]).is_some_and(|output| output.status.success())
}

fn service_active() -> bool {
    systemctl(&["is-active", "--quiet", SERVICE]).is_some_and(|output| output.status.success())
}

fn extension_active() -> bool {
    Command::new("gnome-extensions")
        .args(["info", EXTENSION])
        .output()
        .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("State: ACTIVE"))
}

fn status_problem() -> Option<&'static str> {
    if !service_installed() {
        Some(tr([
            "The background service is not installed, so changes have no effect yet. See \"Run it\" in the README.",
            "Chưa cài dịch vụ chạy nền nên thay đổi chưa có tác dụng. Xem mục \"Cài đặt\" trong README.",
            "后台服务尚未安装，因此更改暂不生效。请参阅 README 中的“运行”部分。",
        ]))
    } else if !extension_active() {
        Some(tr([
            "The GNOME extension is not enabled, so only the \"All apps\" profile works.",
            "GNOME extension chưa bật nên chỉ dùng được profile \"Mọi ứng dụng\".",
            "GNOME 扩展未启用，因此只有“所有应用”配置可用。",
        ]))
    } else {
        None
    }
}

fn service_row(ui: &Rc<Ui>) -> adw::ActionRow {
    let switch = gtk::Switch::builder().valign(gtk::Align::Center).build();
    let installed = service_installed();
    switch.set_active(installed && service_active());
    switch.set_sensitive(installed);

    let toasts = ui.clone();
    switch.connect_state_set(move |switch, on| {
        let action = if on { "enable" } else { "disable" };
        let done = systemctl(&[action, "--now", SERVICE]);
        if !done.is_some_and(|output| output.status.success()) {
            toasts.toast(tr([
                "Could not change the service state.",
                "Không đổi được trạng thái dịch vụ.",
                "无法更改服务状态。",
            ]));
        }
        switch.set_state(service_active());
        glib::Propagation::Stop
    });

    let row = adw::ActionRow::builder()
        .title(tr(["Run in background", "Chạy nền", "后台运行"]))
        .subtitle(tr([
            "Starts when you log in",
            "Tự chạy khi đăng nhập",
            "登录时自动启动",
        ]))
        .build();
    row.add_suffix(&switch);
    row
}

fn language_dropdown(ui: &Rc<Ui>) -> gtk::DropDown {
    let mut labels = vec![tr(["Automatic", "Tự động", "自动"])];
    labels.extend(i18n::CHOICES.iter().map(|(_, name)| *name));
    let dropdown = gtk::DropDown::from_strings(&labels);
    dropdown.set_tooltip_text(Some(tr(["Language", "Ngôn ngữ", "语言"])));
    let saved = i18n::saved_choice()
        .and_then(|language| i18n::CHOICES.iter().position(|(l, _)| *l == language));
    dropdown.set_selected(saved.map_or(0, |index| index as u32 + 1));

    let ui = ui.clone();
    dropdown.connect_selected_notify(move |dropdown| {
        let choice = (dropdown.selected() as usize)
            .checked_sub(1)
            .map(|index| i18n::CHOICES[index].0);
        if let Err(error) = i18n::save_choice(choice) {
            eprintln!("could not save the language: {error}");
        }
        i18n::set_current(choice.unwrap_or_else(i18n::system_language));
        // Every label is set when its widget is built, so the window is rebuilt.
        let ui = ui.clone();
        glib::idle_add_local_once(move || ui.reopen());
    });
    dropdown
}

fn open_store(path: &std::path::Path) -> anyhow::Result<Store> {
    let default_device = readable_mouse_names()
        .into_iter()
        .next()
        .unwrap_or_default();
    Store::open(path, &default_device)
}

fn build_window(app: &adw::Application, store: Store) -> adw::ApplicationWindow {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Clickshift")
        .default_width(1040)
        .default_height(760)
        .build();
    let toasts = adw::ToastOverlay::new();
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    let remove_app = gtk::Button::builder()
        .icon_name("list-remove-symbolic")
        .tooltip_text(tr([
            "Remove this app's profile",
            "Xóa profile của ứng dụng này",
            "删除此应用的配置",
        ]))
        .build();

    let ui = Rc::new(Ui {
        window: window.clone(),
        toasts: toasts.clone(),
        store: RefCell::new(store),
        layer: Cell::new(0),
        app: RefCell::new(None),
        sidebar_apps: RefCell::new(Vec::new()),
        sidebar: sidebar.clone(),
        content: adw::Bin::new(),
        remove_app: remove_app.clone(),
        refreshing_sidebar: Cell::new(false),
    });

    {
        let ui = ui.clone();
        sidebar.connect_row_selected(move |_, row| {
            if ui.refreshing_sidebar.get() {
                return;
            }
            let Some(row) = row else {
                return;
            };
            let entry = ui.sidebar_apps.borrow().get(row.index() as usize).cloned();
            if let Some(entry) = entry {
                ui.remove_app.set_sensitive(entry.is_some());
                *ui.app.borrow_mut() = entry;
                ui.refresh_content();
            }
        });
    }

    let add_app = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(tr(["Add app", "Thêm ứng dụng", "添加应用"]))
        .build();
    {
        let ui = ui.clone();
        add_app.connect_clicked(move |_| ui.show_app_picker());
    }
    {
        let ui = ui.clone();
        remove_app.connect_clicked(move |_| ui.confirm_remove_app());
    }

    let sidebar_header = adw::HeaderBar::new();
    sidebar_header.set_show_title(false);
    sidebar_header.set_show_end_title_buttons(false);
    sidebar_header.pack_start(&add_app);
    sidebar_header.pack_start(&remove_app);

    let sidebar_scroller = gtk::ScrolledWindow::builder()
        .child(&sidebar)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let service_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .margin_top(6)
        .margin_bottom(6)
        .margin_start(6)
        .margin_end(6)
        .build();
    service_list.append(&service_row(&ui));

    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&sidebar_header);
    sidebar_view.set_content(Some(&sidebar_scroller));
    sidebar_view.add_bottom_bar(&service_list);

    let mut layer_names = vec![tr(["Base layer", "Lớp cơ bản", "基础层"]).to_owned()];
    layer_names
        .extend((1..=MAX_LAYER).map(|layer| format!("{} {layer}", tr(["Layer", "Lớp", "层"]))));
    let layer_labels: Vec<&str> = layer_names.iter().map(String::as_str).collect();
    let layer_dropdown = gtk::DropDown::from_strings(&layer_labels);
    layer_dropdown.set_tooltip_text(Some(tr([
        "Layer being edited",
        "Lớp đang chỉnh sửa",
        "正在编辑的层",
    ])));
    {
        let ui = ui.clone();
        layer_dropdown.connect_selected_notify(move |dropdown| {
            ui.layer.set(dropdown.selected() as u8);
            ui.refresh_content();
        });
    }

    let content_header = adw::HeaderBar::new();
    content_header.set_show_start_title_buttons(false);
    content_header.pack_start(&layer_dropdown);
    content_header.pack_end(&language_dropdown(&ui));

    let banner = adw::Banner::new("");
    if let Some(problem) = status_problem() {
        banner.set_title(problem);
        banner.set_revealed(true);
    }

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&content_header);
    content_view.add_top_bar(&banner);
    content_view.set_content(Some(&ui.content));

    let split = gtk::Paned::builder()
        .orientation(gtk::Orientation::Horizontal)
        .start_child(&sidebar_view)
        .end_child(&content_view)
        .position(280)
        .shrink_start_child(false)
        .resize_start_child(false)
        .build();
    toasts.set_child(Some(&split));
    window.set_content(Some(&toasts));

    ui.refresh();
    window
}

fn error_window(app: &adw::Application, message: &str) -> adw::ApplicationWindow {
    let page = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title(tr([
            "Cannot read the config file",
            "Không đọc được file cấu hình",
            "无法读取配置文件",
        ]))
        .description(glib::markup_escape_text(message).as_str())
        .build();
    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&page));
    adw::ApplicationWindow::builder()
        .application(app)
        .title("Clickshift")
        .default_width(640)
        .default_height(420)
        .content(&view)
        .build()
}

fn config_path_from_args() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    match (args.next().as_deref(), args.next()) {
        (None, _) => config::default_path().map_err(|error| format!("{error:#}")),
        (Some("--config"), Some(path)) => Ok(PathBuf::from(path)),
        _ => Err("usage: clickshift-settings [--config PATH]".to_owned()),
    }
}

fn main() -> glib::ExitCode {
    let path = match config_path_from_args() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("{message}");
            return glib::ExitCode::FAILURE;
        }
    };

    i18n::set_current(i18n::saved_choice().unwrap_or_else(i18n::system_language));

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(move |app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        let window = match open_store(&path) {
            Ok(store) => build_window(app, store),
            Err(error) => error_window(app, &format!("{error:#}")),
        };
        window.present();
    });
    // Arguments were handled above; GTK must not try to read them as files.
    app.run_with_args::<&str>(&[])
}
