use tsuro_sign::SignatureStatus;
use iced::widget::{
    button, column, container, image, row, scrollable, svg, text, text_input, tooltip, Space,
};
use iced::{Alignment, Color, Element, Length};

use crate::page::PageNo;
use crate::session::{Message, Ready, Session, Zoom, ZoomFactor};

fn ink() -> Color {
    Color::from_rgb8(0x1c, 0x1c, 0x1a)
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

fn tip<'a>(
    content: impl Into<Element<'a, Message>>,
    label: &'static str,
) -> Element<'a, Message> {
    tooltip::Tooltip::new(content, text(label).size(13), tooltip::Position::Bottom).into()
}

fn open_button() -> Element<'static, Message> {
    tip(
        button(icon!("folder-open")).on_press(Message::PickFile),
        "Abrir PDF",
    )
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
        open_button(),
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
    }

    bar.into()
}

fn empty_drop() -> Element<'static, Message> {
    container(
        column![
            text("Abra um PDF").size(22),
            text("Arraste um arquivo para cá ou clique em Abrir."),
            open_button(),
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
    row![page_pane(ready), signatures_panel(ready)]
        .spacing(12)
        .height(Length::Fill)
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
