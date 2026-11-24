//! Shared section tab strip: a 40 px horizontal underline bar aligned to
//! `content_inset`, with optional leading icons, count badges, and a trailing
//! slot for right-aligned status text or actions.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{scale, tokens};

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type SelectorFn = Rc<dyn Fn() -> String>;

/// One tab inside [`SectionTabs`]. Build with [`section_tab`].
pub struct SectionTab {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    count: Option<usize>,
    aria_label: Option<SharedString>,
    debug_selector: Option<SelectorFn>,
    on_click: Option<ClickHandler>,
}

/// Starts a [`SectionTab`].
pub fn section_tab(id: impl Into<ElementId>, label: impl Into<SharedString>) -> SectionTab {
    SectionTab {
        id: id.into(),
        label: label.into(),
        icon: None,
        count: None,
        aria_label: None,
        debug_selector: None,
        on_click: None,
    }
}

impl SectionTab {
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = Some(count);
        self
    }

    pub fn maybe_count(mut self, count: Option<usize>) -> Self {
        self.count = count;
        self
    }

    pub fn aria_label(mut self, label: impl Into<SharedString>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    pub fn debug_selector(mut self, selector: impl Fn() -> String + 'static) -> Self {
        self.debug_selector = Some(Rc::new(selector));
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

/// A horizontal underline section tab strip. Build with [`section_tabs`].
#[derive(IntoElement)]
pub struct SectionTabs {
    id: ElementId,
    active: usize,
    tabs: Vec<SectionTab>,
    trailing: Vec<AnyElement>,
}

/// Starts a [`SectionTabs`] strip with `active` tab index selected.
pub fn section_tabs(id: impl Into<ElementId>, active: usize) -> SectionTabs {
    SectionTabs {
        id: id.into(),
        active,
        tabs: Vec::new(),
        trailing: Vec::new(),
    }
}

impl SectionTabs {
    pub fn tab(mut self, tab: SectionTab) -> Self {
        self.tabs.push(tab);
        self
    }

    pub fn tabs(mut self, tabs: impl IntoIterator<Item = SectionTab>) -> Self {
        self.tabs.extend(tabs);
        self
    }

    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    pub fn maybe_trailing(mut self, element: Option<impl IntoElement>) -> Self {
        if let Some(el) = element {
            self.trailing.push(el.into_any_element());
        }
        self
    }
}

impl RenderOnce for SectionTabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, muted, accent, border) = (
            theme.foreground,
            theme.muted_foreground,
            tokens::accent(),
            tokens::border_subtle(),
        );
        let active = self.active;
        h_flex()
            .id(self.id)
            .role(Role::TabList)
            .flex_none()
            .w_full()
            .h(scale::toolbar_height())
            .px(scale::content_inset())
            .items_stretch()
            .border_b_1()
            .border_color(border)
            .child(
                // Pull back by 8 px so the first tab's icon/label sits flush
                // on the 16 px content inset while each tab keeps 8 px of hit
                // padding on both sides.
                h_flex()
                    .h_full()
                    .ml(rems(-0.5))
                    .gap_1()
                    .items_stretch()
                    .children(self.tabs.into_iter().enumerate().map(|(ix, tab)| {
                        let selected = ix == active;
                        let aria = tab.aria_label.unwrap_or_else(|| match tab.count {
                            Some(count) => SharedString::from(format!("{}, {count}", tab.label)),
                            None => tab.label.clone(),
                        });
                        h_flex()
                            .id(tab.id)
                            .role(Role::Tab)
                            .aria_label(aria)
                            .when_some(tab.debug_selector, |el, sel| {
                                el.debug_selector(move || sel())
                            })
                            .relative()
                            .h_full()
                            .px_2()
                            .gap_1p5()
                            .items_center()
                            .cursor_pointer()
                            .text_size(scale::body())
                            .text_color(if selected { fg } else { muted })
                            .when(selected, |el| el.font_weight(FontWeight::MEDIUM))
                            .when(!selected, |el| el.hover(move |s| s.text_color(fg)))
                            .when_some(tab.icon, |el, icon| {
                                el.child(
                                    Icon::new(icon)
                                        .small()
                                        .flex_none()
                                        .text_color(if selected { fg } else { muted }),
                                )
                            })
                            .child(div().flex_none().child(tab.label))
                            .when_some(tab.count, |el, count| {
                                el.child(super::count_badge(count, cx))
                            })
                            .when(selected, |el| {
                                el.child(
                                    div()
                                        .absolute()
                                        .left_2()
                                        .right_2()
                                        .bottom_0()
                                        .h(px(2.))
                                        .rounded_t(px(1.))
                                        .bg(accent),
                                )
                            })
                            .when_some(tab.on_click, |el, handler| {
                                el.on_click(move |event, window, cx| handler(event, window, cx))
                            })
                    })),
            )
            .child(div().flex_1())
            .children(self.trailing)
    }
}
