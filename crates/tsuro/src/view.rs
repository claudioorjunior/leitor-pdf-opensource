use iced::widget::{
    button, column, container, image, row, scrollable, svg, text, text_input, tooltip, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};
use tsuro_sign::SignatureStatus;

use crate::browse::{display_path, parent_of, EmptyState, FsEntry};
use crate::page::PageNo;
use crate::session::{Message, Ready, Session, Zoom, ZoomFactor, THUMB_ROW};

pub fn pages_scroll_id() -> scrollable::Id {
    scrollable::Id::new("tsuro-pages")
}

fn paper() -> Color {
    Color::from_rgb8(0xfa, 0xfa, 0xf8)
}

fn ink() -> Color {
    Color::from_rgb8(0x1c, 0x1c, 0x1a)
}

fn line() -> Color {
    Color::from_rgb8(0xe2, 0xe0, 0xd8)
}

fn chip() -> Color {
    Color::from_rgb8(0xee, 0xec, 0xe6)
}

fn chip_hover() -> Color {
    Color::from_rgb8(0xe2, 0xe0, 0xd8)
}

fn chip_press() -> Color {
    Color::from_rgb8(0xd4, 0xd2, 0xc8)
}

fn control_style(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = if active {
            match status {
                button::Status::Hovered | button::Status::Pressed => chip_press(),
                _ => chip_press(),
            }
        } else {
            match status {
                button::Status::Hovered => chip_hover(),
                button::Status::Pressed => chip_press(),
                _ => chip(),
            }
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: ink(),
            border: Border {
                color: line(),
                width: 1.0,
                radius: 6.0.into(),
            },
            shadow: Shadow::default(),
        }
    }
}

fn control(
    btn: iced::widget::button::Button<'_, Message>,
) -> iced::widget::button::Button<'_, Message> {
    control_active(btn, false)
}

fn control_active(
    btn: iced::widget::button::Button<'_, Message>,
    active: bool,
) -> iced::widget::button::Button<'_, Message> {
    btn.padding(Padding::from([7, 8]))
        .style(control_style(active))
}

macro_rules! icon {
    ($file:literal) => {
        svg(svg::Handle::from_memory(include_bytes!(concat!(
            "../assets/icons/",
            $file,
            ".svg"
        ))))
        .width(Length::Fixed(17.0))
        .height(Length::Fixed(17.0))
        .style(|_theme, _status| svg::Style { color: Some(ink()) })
    };
}

fn tip<'a>(content: impl Into<Element<'a, Message>>, label: &'static str) -> Element<'a, Message> {
    tooltip::Tooltip::new(content, text(label).size(13), tooltip::Position::Bottom).into()
}

fn open_button() -> Element<'static, Message> {
    tip(
        control(button(icon!("folder-open")).on_press(Message::PickFile)),
        "Abrir PDF",
    )
}

fn signatures_toggle(ready: &Ready) -> Element<'_, Message> {
    let count = ready.signatures.signatures.len();
    tip(
        control_active(
            button(
                row![icon!("shield-check"), text(format!("{count}")).size(13)]
                    .spacing(4)
                    .align_y(Alignment::Center),
            )
            .on_press(Message::ToggleSignatures),
            ready.signatures_open,
        ),
        "Assinaturas",
    )
}

fn pages_toggle(ready: &Ready) -> Element<'_, Message> {
    tip(
        control_active(
            button(icon!("panel-left")).on_press(Message::TogglePages),
            ready.pages_open,
        ),
        "Páginas",
    )
}

pub fn chrome(session: &Session) -> Element<'_, Message> {
    let body: Element<'_, Message> = match session {
        Session::Empty(empty) => empty_browser(empty),
        Session::Loading { source, .. } => {
            text(format!("Abrindo {}…", source.path().display())).into()
        }
        Session::Failed { message, .. } => column![
            text("Não foi possível abrir o documento").size(20),
            text(message),
        ]
        .spacing(8)
        .into(),
        Session::Ready(ready) => ready_body(ready),
    };

    container(
        column![toolbar(session), body]
            .spacing(8)
            .padding(12)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(paper())),
        text_color: Some(ink()),
        ..container::Style::default()
    })
    .into()
}

fn toolbar(session: &Session) -> Element<'_, Message> {
    let mut bar = row![
        open_button(),
        control(button("Fechar").on_press(Message::Close)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if let Session::Ready(ready) = session {
        let n = ready.page_count().max(1);
        let idx = ready.visible.index();
        let prev = idx.saturating_sub(1);
        let next = (idx + 1).min(n - 1);
        bar = bar.push(pages_toggle(ready));
        bar = bar.push(control(
            button("Anterior").on_press(Message::SetPage(PageNo::from_index(prev))),
        ));
        bar = bar.push(text(format!("Página {} / {n}", idx + 1)));
        bar = bar.push(control(
            button("Próxima").on_press(Message::SetPage(PageNo::from_index(next))),
        ));
        bar = bar.push(control(
            button("Ajustar à largura").on_press(Message::SetZoom(Zoom::Width)),
        ));
        bar = bar.push(control(
            button("Página").on_press(Message::SetZoom(Zoom::Page)),
        ));
        let current = match ready.zoom {
            Zoom::Manual(z) => z.get(),
            Zoom::Width | Zoom::Page => ready
                .zoom
                .scale(ready.viewport(), ready.media(ready.visible))
                .factor(),
        };
        bar = bar.push(control(button("−").on_press(Message::SetZoom(
            Zoom::Manual(ZoomFactor::new(current / 1.1)),
        ))));
        bar = bar.push(text(format!("{:.0}%", current * 100.0)));
        bar = bar.push(control(button("+").on_press(Message::SetZoom(
            Zoom::Manual(ZoomFactor::new(current * 1.1)),
        ))));
        bar = bar.push(
            text_input("Buscar", ready.search.query())
                .on_input(Message::SearchChanged)
                .width(Length::Fixed(220.0)),
        );
        bar = bar.push(text(format!("{} ocorrências", ready.search.hits().len())));
        if ready.selection_plain_text().is_some() {
            bar = bar.push(control(button("Copiar").on_press(Message::CopySelection)));
        }
        bar = bar.push(signatures_toggle(ready));
    }

    bar.into()
}

fn entry_style() -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    |_theme, status| {
        let background = match status {
            button::Status::Hovered | button::Status::Pressed => chip_hover(),
            _ => chip(),
        };
        button::Style {
            background: Some(Background::Color(background)),
            text_color: ink(),
            border: Border {
                color: line(),
                width: 1.0,
                radius: 6.0.into(),
            },
            shadow: Shadow::default(),
        }
    }
}

fn empty_browser(empty: &EmptyState) -> Element<'_, Message> {
    let mut path_row = row![].spacing(6).align_y(Alignment::Center);
    if empty.cwd.is_some() {
        let parent = empty.cwd.as_deref().and_then(parent_of);
        path_row = path_row.push(tip(
            button(icon!("chevron-left"))
                .padding(Padding::from([6, 8]))
                .style(entry_style())
                .on_press(Message::BrowseTo(parent)),
            "Voltar",
        ));
    }
    path_row = path_row.push(text(display_path(empty.cwd.as_deref())).size(14));

    let mut listing = column![].spacing(4);
    if let Some(err) = &empty.listing_error {
        listing = listing.push(text(err).size(13));
    } else if empty.listing.is_empty() {
        listing = listing.push(text("Nenhuma pasta ou PDF aqui.").size(13));
    } else {
        for entry in &empty.listing {
            listing = listing.push(entry_row(entry));
        }
    }

    let mut recents = column![].spacing(4);
    if empty.recents.is_empty() {
        recents = recents.push(text("Nenhum arquivo recente.").size(13));
    } else {
        for path in &empty.recents {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            recents = recents.push(
                button(
                    row![icon!("file-text"), text(name).size(14)]
                        .spacing(8)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(Padding::from([7, 8]))
                .style(entry_style())
                .on_press(Message::OpenRecent(path.clone())),
            );
        }
    }

    container(
        column![
            path_row,
            scrollable(listing)
                .width(Length::Fill)
                .height(Length::FillPortion(3)),
            text("Últimos arquivos").size(16),
            scrollable(recents)
                .width(Length::Fill)
                .height(Length::FillPortion(2)),
        ]
        .spacing(10)
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(paper())),
        text_color: Some(ink()),
        ..container::Style::default()
    })
    .into()
}

fn entry_row(entry: &FsEntry) -> Element<'static, Message> {
    let message = if entry.is_dir {
        Message::BrowseTo(Some(entry.path.clone()))
    } else {
        Message::OpenRecent(entry.path.clone())
    };
    let glyph: Element<'static, Message> = if entry.is_dir {
        icon!("folder").into()
    } else {
        icon!("file-text").into()
    };
    button(
        row![glyph, text(entry.name.clone()).size(14)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([7, 8]))
    .style(entry_style())
    .on_press(message)
    .into()
}

fn ready_body(ready: &Ready) -> Element<'_, Message> {
    let mut panes = row![].spacing(12).height(Length::Fill);
    if ready.pages_open {
        panes = panes.push(pages_panel(ready));
    }
    panes = panes.push(page_pane(ready));
    if ready.signatures_open {
        panes = panes.push(signatures_panel(ready));
    }
    panes.into()
}

fn pages_panel(ready: &Ready) -> Element<'_, Message> {
    let window = ready.thumb_page_window();
    let start = window.first().map(|page| page.index()).unwrap_or(0);
    let end = window.last().map(|page| page.index() + 1).unwrap_or(0);
    let mut col = column![].spacing(0);
    if start > 0 {
        col = col.push(Space::with_height(Length::Fixed(start as f32 * THUMB_ROW)));
    }
    for i in start..end {
        let page = PageNo::from_index(i);
        let preview: Element<'_, Message> = match ready.thumb_surface(page) {
            Some(surface) => {
                let handle = image::Handle::from_rgba(
                    surface.bitmap.width,
                    surface.bitmap.height,
                    surface.bitmap.rgba.clone(),
                );
                image(handle).width(Length::Fixed(120.0)).into()
            }
            None => container(text("…").size(13))
                .width(Length::Fixed(120.0))
                .height(Length::Fixed(150.0))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(|_| container::Style {
                    background: Some(Background::Color(chip())),
                    border: Border {
                        color: line(),
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    text_color: Some(ink()),
                    ..container::Style::default()
                })
                .into(),
        };
        col = col.push(
            container(control_active(
                button(
                    column![preview, text(format!("{}", i + 1)).size(12)]
                        .spacing(4)
                        .align_x(Alignment::Center),
                )
                .on_press(Message::SetPage(page))
                .width(Length::Fill),
                ready.visible == page,
            ))
            .width(Length::Fill)
            .height(Length::Fixed(THUMB_ROW)),
        );
    }
    let remaining = ready.page_count().saturating_sub(end);
    if remaining > 0 {
        col = col.push(Space::with_height(Length::Fixed(
            remaining as f32 * THUMB_ROW,
        )));
    }
    container(
        scrollable(col)
            .id(pages_scroll_id())
            .on_scroll(|viewport| Message::PagesScrolled(viewport.absolute_offset().y))
            .height(Length::Fill),
    )
    .width(Length::Fixed(156.0))
    .height(Length::Fill)
    .style(|_| container::Style {
        background: Some(Background::Color(paper())),
        border: Border {
            color: line(),
            width: 1.0,
            radius: 6.0.into(),
        },
        text_color: Some(ink()),
        ..container::Style::default()
    })
    .into()
}

fn page_pane(ready: &Ready) -> Element<'_, Message> {
    let page_view: Element<'_, Message> = match ready.visible_surface() {
        Some(surface) => {
            let handle = image::Handle::from_rgba(
                surface.bitmap.width,
                surface.bitmap.height,
                surface.bitmap.rgba.clone(),
            );
            image(handle).width(Length::Fill).into()
        }
        None if ready.visible_render_failed() => {
            text("Não foi possível renderizar esta página.").into()
        }
        None => text("Renderizando página…").into(),
    };

    scrollable(
        container(page_view)
            .width(Length::Fill)
            .center_x(Length::Fill),
    )
    .width(Length::FillPortion(3))
    .height(Length::Fill)
    .into()
}

fn signatures_panel(ready: &Ready) -> Element<'_, Message> {
    let mut col = column![text("Assinaturas").size(18)].spacing(8);
    if ready.signatures.signatures.is_empty() {
        col = col.push(text("Nenhuma assinatura neste arquivo."));
    } else {
        for sig in &ready.signatures.signatures {
            let name = sig
                .signer_name
                .as_deref()
                .or(sig
                    .certificate
                    .as_ref()
                    .and_then(|c| c.common_name.as_deref()))
                .unwrap_or("Assinante");
            col = col.push(
                column![
                    text(name).size(16),
                    text(status_label(sig.status)),
                    text(&sig.status_detail),
                ]
                .spacing(2),
            );
        }
    }
    col = col.push(Space::with_height(Length::Fixed(8.0)));
    scrollable(col)
        .width(Length::FillPortion(1))
        .height(Length::Fill)
        .into()
}

fn status_label(status: SignatureStatus) -> &'static str {
    match status {
        SignatureStatus::Valid => "Válida",
        SignatureStatus::IntactButUntrusted => "Íntegra (sem confiança pública)",
        SignatureStatus::DocumentModified => "Documento alterado",
        SignatureStatus::Invalid => "Inválida",
        SignatureStatus::Unsupported => "Não suportada",
        SignatureStatus::CertificateExpired => "Certificado expirado",
        SignatureStatus::CertificateNotYetValid => "Certificado ainda não válido",
    }
}
