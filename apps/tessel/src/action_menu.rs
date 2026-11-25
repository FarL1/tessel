//! 动作菜单（Ctrl+K）：针对当前选中的记录，列出可以做的事和可用的转换。
//!
//! 菜单是弹窗里的一层浮层，不是独立窗口：焦点移到菜单自己的搜索框里，
//! 键位上下文是 `ClipPopup > ActionMenu`，所以 ↑ ↓ Enter Esc 在菜单打开时作用于菜单。

use gpui_kit::assets::IconName;
use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use tessel_core::i18n::tr;
use tessel_core::transform::{self, Transform};
use tessel_core::trf;
use tessel_core::{Clip, Kind};

use crate::design::{self, Palette};

actions!(action_menu, [MenuPrev, MenuNext, MenuConfirm, MenuClose]);

pub const CONTEXT: &str = "ActionMenu";
const MENU_W: f32 = 380.;

/// 菜单打开时的键位。必须在弹窗的键位之后注册，同一深度以后注册的为准。
pub fn bind_keys(cx: &mut App) {
    let mut bindings = Vec::new();
    for ctx in ["ActionMenu > Input", CONTEXT] {
        let c = Some(ctx);
        bindings.extend([
            KeyBinding::new("up", MenuPrev, c),
            KeyBinding::new("down", MenuNext, c),
            KeyBinding::new("enter", MenuConfirm, c),
            KeyBinding::new("escape", MenuClose, c),
            KeyBinding::new("ctrl-k", MenuClose, c),
        ]);
    }
    cx.bind_keys(bindings);
}

// 默认图标包之外用到的 Lucide 图标，只嵌入这几个
gpui_kit::assets::icon_assets!(
    ExtraIcons,
    [
        ClipboardPaste,
        ClipboardCopy,
        Type,
        Braces,
        Minimize2,
        ShieldCheck,
        Link,
        Eraser,
        TextWrap,
        CaseUpper,
        CaseLower,
        Trash,
        Command,
        Ellipsis,
        Copy,
        Star,
        StarOff,
        ExternalLink,
        Check,
        ListOrdered,
        ListPlus,
        ListX,
        SquarePen,
        StickyNote,
        Scissors,
        SquarePlus,
    ]
);

/// 组件库自带图标 + 动作菜单额外用到的图标
pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(bytes) = ExtraIcons.load(path)? {
            return Ok(Some(bytes));
        }
        gpui_kit::assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut all = gpui_kit::assets::Assets.list(path)?;
        all.extend(ExtraIcons.list(path)?);
        Ok(all)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    Paste,
    PastePlain,
    CopyOnly,
    PastePaths,
    Open,
    Transform(Transform),
    TogglePin,
    Delete,
    Edit,
    EditNote,
    ToggleSnippet,
    NewSnippet,
    ToggleQueue,
    ClearQueue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Group {
    Paste,
    Transform,
    Manage,
}

pub struct Entry {
    pub action: MenuAction,
    label: SharedString,
    /// 转换结果的预览，或者一句说明
    detail: Option<SharedString>,
    icon: IconName,
    keys: &'static [&'static str],
    group: Group,
    /// 搜索用：小写的标题 + 关键词
    haystack: String,
    /// 转换的结果（执行时直接用，不再重新计算）
    pub output: Option<String>,
}

impl Entry {
    fn new(
        action: MenuAction,
        label: impl Into<SharedString>,
        icon: IconName,
        group: Group,
    ) -> Self {
        let label = label.into();
        Self {
            action,
            haystack: label.to_lowercase(),
            label,
            detail: None,
            icon,
            keys: &[],
            group,
            output: None,
        }
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn keys(mut self, keys: &'static [&'static str]) -> Self {
        self.keys = keys;
        self
    }

    fn keywords(mut self, words: &str) -> Self {
        self.haystack.push(' ');
        self.haystack.push_str(&words.to_lowercase());
        self
    }
}

/// 转换结果的单行预览。多行结果把开头几行拼起来（格式化后的 JSON 第一行只有一个 `{`）
fn summarize(output: &str) -> String {
    const MAX: usize = 64;
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let joined = lines.join(" ");
    let mut s: String = joined.chars().take(MAX).collect();
    if joined.chars().count() > MAX {
        s.push('…');
    }
    if lines.len() > 1 {
        s = trf!("{} 行 · {s}", "{} lines · {s}", lines.len());
    }
    s
}

pub struct ActionMenu {
    /// 菜单针对的记录
    pub uuid: String,
    title: String,
    entries: Vec<Entry>,
    /// 搜索过滤后可见的条目（`entries` 的下标）
    visible: Vec<usize>,
    selected: usize,
    scroll: ScrollHandle,
    /// 右键打开时菜单出现在鼠标位置（窗口坐标）；键盘打开时为空，显示在右下角
    anchor: Option<Point<Pixels>>,
}

impl ActionMenu {
    /// `queued`：这条在粘贴队列里的位置；`queue_len`：队列里一共几条
    pub fn new(clip: &Clip, queued: Option<usize>, queue_len: usize) -> Self {
        use MenuAction as A;
        let paste = Entry::new(
            A::Paste,
            tr("粘贴", "Paste"),
            IconName::ClipboardPaste,
            Group::Paste,
        )
        .keys(&["↵"])
        .keywords("paste");
        let mut entries = vec![if clip.has_rich {
            paste.detail(tr("保留格式", "Keeps formatting"))
        } else {
            paste
        }];
        if clip.has_rich {
            entries.push(
                Entry::new(
                    A::PastePlain,
                    tr("粘贴为纯文本", "Paste as plain text"),
                    IconName::Type,
                    Group::Paste,
                )
                .keys(&["Shift", "↵"])
                .keywords("plain text 格式"),
            );
        }
        entries.push(
            Entry::new(
                A::CopyOnly,
                tr("只复制，不粘贴", "Copy without pasting"),
                IconName::ClipboardCopy,
                Group::Paste,
            )
            .detail(tr(
                "放到剪贴板，之后自己粘贴",
                "Puts it on the clipboard for you to paste",
            ))
            .keywords("copy"),
        );
        if clip.kind == Kind::File {
            let n = clip.files().count();
            entries.push(
                Entry::new(
                    A::PastePaths,
                    tr("粘贴文件路径", "Paste file paths"),
                    IconName::Type,
                    Group::Paste,
                )
                .detail(if n > 1 {
                    trf!("{} 个路径，每行一个", "{} paths, one per line", n)
                } else {
                    clip.files().next().unwrap_or_default().to_string()
                })
                .keywords("path text"),
            );
        }
        let open = match clip.kind {
            Kind::Link => Some((
                tr("在浏览器中打开", "Open in browser"),
                IconName::ExternalLink,
            )),
            Kind::File => Some((
                tr("在资源管理器中显示", "Show in File Explorer"),
                IconName::FolderOpen,
            )),
            Kind::Image => Some((
                tr("用系统查看器打开", "Open in default viewer"),
                IconName::ExternalLink,
            )),
            _ => None,
        };
        if let Some((label, icon)) = open {
            entries.push(
                Entry::new(A::Open, label, icon, Group::Paste)
                    .keys(&["Ctrl", "O"])
                    .keywords("open"),
            );
        }

        for (t, output) in transform::available(clip) {
            let icon = match t {
                Transform::FormatJson => IconName::Braces,
                Transform::MinifyJson => IconName::Minimize2,
                Transform::StripTracking => IconName::ShieldCheck,
                Transform::UrlDecode => IconName::Link,
                Transform::ColorHex | Transform::ColorRgb | Transform::ColorHsl => {
                    IconName::Palette
                }
                Transform::TidyWhitespace => IconName::Eraser,
                Transform::JoinLines => IconName::TextWrap,
                Transform::Uppercase => IconName::CaseUpper,
                Transform::Lowercase => IconName::CaseLower,
            };
            let mut entry = Entry::new(A::Transform(t), t.label(), icon, Group::Transform)
                .detail(summarize(&output))
                .keywords(t.keywords());
            entry.output = Some(output);
            entries.push(entry);
        }

        if !matches!(clip.kind, Kind::Image | Kind::File) {
            entries.push(
                Entry::new(
                    A::Edit,
                    tr("编辑文本", "Edit text"),
                    IconName::SquarePen,
                    Group::Manage,
                )
                .detail(tr("改完保存，替换这条记录", "Save to replace this item"))
                .keys(&["Ctrl", "E"])
                .keywords("edit modify 修改"),
            );
        }
        entries.push(
            Entry::new(
                A::EditNote,
                if clip.note.is_some() {
                    tr("修改备注", "Edit note")
                } else {
                    tr("添加备注", "Add note")
                },
                IconName::StickyNote,
                Group::Manage,
            )
            .detail(match &clip.note {
                Some(note) => note.clone(),
                None => tr(
                    "给这条起个名字，用 #标签 分类，之后搜得到",
                    "Name it or group it with #tags to find it later",
                )
                .to_string(),
            })
            .keys(&["F2"])
            .keywords("note tag rename 备注 标签 重命名"),
        );
        if !matches!(clip.kind, Kind::Image | Kind::File) {
            entries.push(
                Entry::new(
                    A::ToggleSnippet,
                    if clip.snippet {
                        tr("取消片段", "Unmark as snippet")
                    } else {
                        tr("存为片段", "Save as snippet")
                    },
                    IconName::Scissors,
                    Group::Manage,
                )
                .detail(if clip.snippet {
                    tr("变回普通记录", "Turn it back into a normal item")
                } else {
                    tr(
                        "可以用 {date} {cursor} 等变量，永远保留",
                        "Supports {date}, {cursor}… and is kept forever",
                    )
                })
                .keywords("snippet template 片段 模板 变量"),
            );
        }
        entries.push(
            Entry::new(
                A::NewSnippet,
                tr("新建片段", "New snippet"),
                IconName::SquarePlus,
                Group::Manage,
            )
            .keys(&["Ctrl", "N"])
            .keywords("new snippet template 新建 片段 模板"),
        );
        entries.push(
            match queued {
                Some(i) => Entry::new(
                    A::ToggleQueue,
                    tr("移出粘贴队列", "Remove from paste queue"),
                    IconName::ListX,
                    Group::Manage,
                )
                .detail(trf!(
                    "现在排在第 {} 条",
                    "Currently #{} in the queue",
                    i + 1
                )),
                None => Entry::new(
                    A::ToggleQueue,
                    tr("加入粘贴队列", "Add to paste queue"),
                    IconName::ListPlus,
                    Group::Manage,
                )
                .detail(trf!(
                    "排在第 {} 条，之后每按一次队列热键粘贴一条",
                    "Becomes #{}; each press of the queue hotkey pastes the next one",
                    queue_len + 1
                )),
            }
            .keys(&["Ctrl", "Q"])
            .keywords("queue 队列 排队 依次"),
        );
        if queue_len > 0 {
            entries.push(
                Entry::new(
                    A::ClearQueue,
                    tr("清空粘贴队列", "Clear paste queue"),
                    IconName::ListOrdered,
                    Group::Manage,
                )
                .detail(trf!("队列里有 {queue_len} 条", "{queue_len} in the queue"))
                .keywords("queue clear 队列"),
            );
        }
        entries.push(
            Entry::new(
                A::TogglePin,
                if clip.pinned {
                    tr("取消收藏", "Unpin")
                } else {
                    tr("收藏", "Pin")
                },
                if clip.pinned {
                    IconName::StarOff
                } else {
                    IconName::Star
                },
                Group::Manage,
            )
            .keys(&["Ctrl", "P"])
            .keywords("pin star"),
        );
        entries.push(
            Entry::new(
                A::Delete,
                tr("删除", "Delete"),
                IconName::Trash,
                Group::Manage,
            )
            .keys(&["Shift", "Del"])
            .keywords("delete remove"),
        );

        let visible = (0..entries.len()).collect();
        Self {
            uuid: clip.uuid.clone(),
            title: clip.title(),
            entries,
            visible,
            selected: 0,
            scroll: ScrollHandle::new(),
            anchor: None,
        }
    }

    /// 在鼠标位置打开（右键菜单）
    pub fn at(mut self, anchor: Point<Pixels>) -> Self {
        self.anchor = Some(anchor);
        self
    }

    pub fn set_query(&mut self, query: &str) {
        let terms: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| terms.iter().all(|t| e.haystack.contains(t.as_str())))
            .map(|(i, _)| i)
            .collect();
        self.selected = 0;
        self.scroll.scroll_to_item(0);
    }

    pub fn move_selection(&mut self, delta: isize) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).rem_euclid(len as isize) as usize;
        self.scroll.scroll_to_item(self.selected);
    }

    pub fn select(&mut self, pos: usize) {
        if pos < self.visible.len() {
            self.selected = pos;
        }
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entries.get(*self.visible.get(self.selected)?)
    }

    /// 渲染菜单浮层。`on_pick(pos)`：鼠标点了第 pos 个可见条目。
    pub fn render(
        &self,
        search: &Entity<InputState>,
        p: &Palette,
        on_pick: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let p = *p;
        let on_pick = std::rc::Rc::new(on_pick);
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut prev_group = None;
        for (pos, &ix) in self.visible.iter().enumerate() {
            let e = &self.entries[ix];
            // 分组之间的分隔线和条目放在同一个子元素里，滚动时子元素下标 = 可见条目下标
            let new_group = prev_group.is_some_and(|g| g != e.group);
            prev_group = Some(e.group);
            let selected = pos == self.selected;
            let on_pick = on_pick.clone();
            let row = h_flex()
                .id(("menu-entry", pos))
                .mx_1p5()
                .px_2()
                .py_1p5()
                .gap_2p5()
                .rounded(design::RADIUS_SM)
                .cursor_pointer()
                .when(selected, |d| d.bg(p.selected))
                .when(!selected, |d| d.hover(move |s| s.bg(p.hover)))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    cx.stop_propagation();
                    on_pick(pos, window, cx);
                })
                .child(Icon::new(e.icon).small().text_color(if selected {
                    p.accent
                } else {
                    p.text_muted
                }))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().text_color(p.text).child(e.label.clone()))
                        .when_some(e.detail.clone(), |d, detail| {
                            d.child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_faint)
                                    .truncate()
                                    .child(detail),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .children(e.keys.iter().map(|k| crate::ui::kbd(*k, &p))),
                );
            rows.push(
                v_flex()
                    .when(new_group, |d| {
                        d.child(div().mx_3().my_1().h(px(1.)).bg(p.divider))
                    })
                    .child(row)
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .py_6()
                    .text_center()
                    .text_xs()
                    .text_color(p.text_faint)
                    .child(tr("没有匹配的动作", "No matching actions"))
                    .into_any_element(),
            );
        }

        v_flex()
            .id("action-menu")
            .key_context(CONTEXT)
            .absolute()
            .w(px(MENU_W))
            .map(|d| match self.anchor {
                None => d.right(px(14.)).bottom(px(46.)).max_h(px(400.)),
                // 鼠标位置：横向不超出窗口；在上半部分点的向下展开，下半部分点的向上展开
                Some(at) => {
                    let (win_w, win_h) = (design::WINDOW_W, design::WINDOW_H);
                    let left = f32::from(at.x).clamp(8., win_w - 8. - MENU_W);
                    let y = f32::from(at.y);
                    let d = d.left(px(left));
                    if y < win_h / 2. {
                        d.top(px(y + 2.)).max_h(px((win_h - y - 12.).min(400.)))
                    } else {
                        d.bottom(px(win_h - y + 2.)).max_h(px((y - 12.).min(400.)))
                    }
                }
            })
            .rounded(design::RADIUS_LG)
            .bg(p.popover)
            .border_1()
            .border_color(p.border)
            .shadow_lg()
            .overflow_hidden()
            // 点菜单内部不要触发外层"点击空白处关闭"
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                h_flex()
                    .px_3()
                    .pt_2p5()
                    .pb_1()
                    .gap_1p5()
                    .text_xs()
                    .text_color(p.text_faint)
                    .child(Icon::new(IconName::Command).xsmall())
                    .child(div().flex_none().child(tr("动作", "Actions")))
                    .child(div().flex_none().child("·"))
                    .child(div().min_w_0().truncate().child(self.title.clone())),
            )
            .child(
                div()
                    .mx_2()
                    .mb_1()
                    .px_2()
                    .h(px(34.))
                    .flex()
                    .items_center()
                    .rounded(design::RADIUS_MD)
                    .bg(p.search_bg)
                    .border_1()
                    .border_color(p.search_border)
                    .child(Input::new(search).appearance(false).small()),
            )
            .child(
                div()
                    .id("menu-entries")
                    .flex_1()
                    .min_h_0()
                    .pb_1p5()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows),
            )
    }
}
