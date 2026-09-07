use iced::widget::{
    button, column, container, image, row, scrollable, svg, text, text_input, tooltip, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding, Shadow};
use tsuro_sign::SignatureStatus;

use crate::page::PageNo;
use crate::session::{Message, Ready, Session, Zoom, ZoomFactor};

pub fn pages_scroll_id() -> scrollable::Id {
    scrollable::Id::new("tsuro-pages")
}

pub fn chrome(session: &Session) -> Element<'_, Message> {
    let body: Element<'_, Message> = match session {
        Session::Empty => empty_drop(),
        Session::Loading { source } => text(format!("Abrindo {}…", source.path().display())).into(),
        Session::Failed { message, .. } => column![
            text("Não foi possível abrir o documento").size(20),
            text(message),
        ]
        .spacing(8)
        .into(),
        Session::Ready(ready) => ready_body(ready),
    };

    column![toolbar(session), body]
        .spacing(8)
        .padding(12)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn toolbar(session: &Session) -> Element<'_, Message> {
    let mut bar = row![
        button("Abrir").on_press(Message::PickFile),
        button("Fechar").on_press(Message::Close),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if let Session::Ready(ready) = session {
        let n = ready.page_count().max(1);
        let idx = ready.visible.index();
        let prev = idx.saturating_sub(1);
        let next = (idx + 1).min(n - 1);
        bar = bar.push(button("Anterior").on_press(Message::SetPage(PageNo::from_index(prev))));
        bar = bar.push(text(format!("Página {} / {n}", idx + 1)));
        bar = bar.push(button("Próxima").on_press(Message::SetPage(PageNo::from_index(next))));
        bar = bar.push(button("Ajustar à largura").on_press(Message::SetZoom(Zoom::Width)));
        bar = bar.push(button("Página").on_press(Message::SetZoom(Zoom::Page)));
        let current = match ready.zoom {
            Zoom::Manual(z) => z.get(),
            Zoom::Width | Zoom::Page => ready
                .zoom
                .scale(ready.viewport(), ready.media(ready.visible))
                .factor(),
        };
        bar = bar.push(
            button("−").on_press(Message::SetZoom(Zoom::Manual(ZoomFactor::new(current / 1.1)))),
        );
        bar = bar.push(text(format!("{:.0}%", current * 100.0)));
        bar = bar.push(
            button("+").on_press(Message::SetZoom(Zoom::Manual(ZoomFactor::new(current * 1.1)))),
        );
        bar = bar.push(
            text_input("Buscar", ready.search.query())
                .on_input(Message::SearchChanged)
                .width(Length::Fixed(220.0)),
        );
        bar = bar.push(text(format!("{} ocorrências", ready.search.hits().len())));
        if ready.selection_plain_text().is_some() {
            bar = bar.push(button("Copiar").on_press(Message::CopySelection));
        }
        bar = bar.push(tip(
            button(pages_icon())
                .on_press(Message::TogglePages)
                .padding(Padding::from([7, 8]))
                .style(pages_control_style(ready.pages_open)),
            "Páginas",
        ));
    }

    bar.into()
}

fn empty_drop() -> Element<'static, Message> {
    container(
        column![
            text("Abra um PDF").size(22),
            text("Arraste um arquivo para cá ou clique em Abrir."),
            button("Abrir").on_press(Message::PickFile),
        ]
        .spacing(10)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn ready_body(ready: &Ready) -> Element<'_, Message> {
    let mut panes = row![].spacing(12).height(Length::Fill);
    if ready.pages_open {
        panes = panes.push(pages_panel(ready));
    }
    panes = panes.push(page_pane(ready));
    panes = panes.push(signatures_panel(ready));
    panes.into()
}

fn pages_panel(ready: &Ready) -> Element<'_, Message> {
    let mut col = column![].spacing(8);
    for i in 0..ready.page_count() {
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
            button(
                column![preview, text(format!("{}", i + 1)).size(12)]
                    .spacing(4)
                    .align_x(Alignment::Center),
            )
            .on_press(Message::SetPage(page))
            .padding(Padding::from([7, 8]))
            .style(pages_control_style(ready.visible == page))
            .width(Length::Fill),
        );
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

fn pages_icon() -> svg::Svg<'static> {
    svg(svg::Handle::from_memory(include_bytes!(
        "../assets/icons/panel-left.svg"
    )))
    .width(Length::Fixed(17.0))
    .height(Length::Fixed(17.0))
    .style(|_theme, _status| svg::Style { color: Some(ink()) })
}

fn pages_control_style(active: bool) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let background = if active {
            chip_press()
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

fn tip<'a>(content: impl Into<Element<'a, Message>>, label: &'static str) -> Element<'a, Message> {
    tooltip::Tooltip::new(content, text(label).size(13), tooltip::Position::Bottom).into()
}
