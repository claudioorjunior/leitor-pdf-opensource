use iced::widget::{
    button, column, container, image, mouse_area, pick_list, row, scrollable, stack, svg, text,
    text_input, tooltip, Space,
};
use iced::{Alignment, Background, Border, Color, Element, Length, Padding};
use tsuro_sign::SignatureStatus;

use crate::browse::{EmptyState, FsEntry};
use crate::kiri::{self, Theme, Tokens};
use crate::page::PageNo;
use crate::print::{PrintOrientation, MAX_COPIES};
use crate::session::{
    Message, NavCmd, PrintDialog, RangeMode, Ready, Session, Zoom, ZoomFactor, THUMB_ROW,
};

/// Altura do chrome Kiri: toolbar 36px + progresso 2px + respiro.
pub const CHROME_HEIGHT: f32 = 46.0;

pub fn pages_scroll_id() -> scrollable::Id {
    scrollable::Id::new("tsuro-pages")
}

pub fn chrome(session: &Session, theme: Theme) -> Element<'_, Message> {
    let t = Tokens::for_theme(theme);
    let body: Element<'_, Message> = match session {
        Session::Empty(empty) => empty_browser(empty, t),
        Session::Loading { source, .. } => {
            text(format!("Abrindo {}…", source.path().display())).into()
        }
        Session::Failed { message, .. } => column![
            text("Não foi possível abrir o documento").size(20),
            text(message),
        ]
        .spacing(8)
        .into(),
        Session::Ready(ready) => ready_body(ready, t),
    };

    // Toolbar 36px + progresso 2px + respiro 8px = `CHROME_HEIGHT` (46px).
    let mut col = column![topbar(session, t)];
    if let Session::Ready(ready) = session {
        col = col.push(progress(ready, t));
    }
    col = col.push(body);
    let main = container(
        col.spacing(0)
            .padding(4)
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(t.bg)),
        text_color: Some(t.ink),
        ..container::Style::default()
    });
    match session {
        // Modal de impressão captura tudo; menu ⋯ nunca abre junto (fecha ao abrir).
        Session::Ready(ready) if ready.print_dialog.is_some() => {
            let dialog = ready.print_dialog.as_ref().expect("checked above");
            stack![main, print_layer(ready, dialog, t)].into()
        }
        // Overlay visual: só os botões capturam clique, o resto atravessa.
        Session::Ready(ready) if ready.overflow_open => {
            stack![main, overflow_layer(ready, t)].into()
        }
        _ => main.into(),
    }
}

// Lucide restante (sem par Ori v1): `copy`, `x`, `folder`, `file-text`, `chevron-left`
// (empty state). Toolbar usa `kiri::ori!` — cor fixa no SVG, sem `.style()`.
macro_rules! icon {
    ($t:expr, $file:literal) => {
        svg(svg::Handle::from_memory(include_bytes!(concat!(
            "../assets/icons/",
            $file,
            ".svg"
        ))))
        .width(Length::Fixed(17.0))
        .height(Length::Fixed(17.0))
        .style(move |_theme, _status| svg::Style {
            color: Some($t.ink),
        })
    };
}

fn control_style(
    t: Tokens,
    active: bool,
) -> impl Fn(&iced::Theme, button::Status) -> button::Style {
    kiri::ibtn_style(t, active)
}

fn control(
    t: Tokens,
    btn: iced::widget::button::Button<'_, Message>,
) -> iced::widget::button::Button<'_, Message> {
    control_active(t, btn, false)
}

fn control_active(
    t: Tokens,
    btn: iced::widget::button::Button<'_, Message>,
    active: bool,
) -> iced::widget::button::Button<'_, Message> {
    btn.padding(Padding::from([9, 10]))
        .style(control_style(t, active))
}

/// Botão-ícone 30×30 para dentro de segmentos (`kiri::seg_style` + padding 2).
fn control_seg(
    t: Tokens,
    btn: iced::widget::button::Button<'_, Message>,
) -> iced::widget::button::Button<'_, Message> {
    btn.padding(Padding::from([5, 6]))
        .style(control_style(t, false))
}

fn tip<'a>(content: impl Into<Element<'a, Message>>, label: &'static str) -> Element<'a, Message> {
    tooltip::Tooltip::new(content, text(label).size(13), tooltip::Position::Bottom).into()
}

fn open_button(t: Tokens) -> Element<'static, Message> {
    tip(
        control(
            t,
            button(kiri::ori!("folder-open")).on_press(Message::PickFile),
        ),
        "Abrir PDF",
    )
}

fn home_button(t: Tokens) -> Element<'static, Message> {
    tip(
        control(t, button(kiri::ori!("home")).on_press(Message::Close)),
        "Início",
    )
}

/// Moldura da toolbar Kiri: 36px, fundo `chrome`, respiro horizontal 8px.
fn toolbar_frame(t: Tokens, content: Element<'_, Message>) -> Element<'_, Message> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(36.0))
        .padding(Padding::from([0, 8]))
        .style(move |_| container::Style {
            background: Some(Background::Color(t.chrome)),
            ..container::Style::default()
        })
        .into()
}

/// Barra única Kiri (`Session::Ready`): abrir │ pílula │ zoom │ painéis │ ⋯.
fn topbar(session: &Session, t: Tokens) -> Element<'_, Message> {
    if let Session::Ready(ready) = session {
        let n = ready.page_count().max(1);
        let pill = container(
            row![
                kiri::ori_small!("search"),
                text_input("Buscar", ready.search.query())
                    .on_input(Message::SearchChanged)
                    .width(Length::Fixed(200.0)),
                row![
                    tip(
                        text_input("Página", ready.page_input())
                            .on_input(Message::PageInput)
                            .on_submit(Message::PageSubmit)
                            .width(Length::Fixed(48.0))
                            .padding([4, 6])
                            .size(12),
                        "Ir para página (Enter confirma)",
                    ),
                    text(format!("/{n}")).size(12).color(t.muted),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                container(
                    row![
                        tip(
                            control_seg(
                                t,
                                button(kiri::ori!("chevron-left"))
                                    .on_press(Message::Nav(NavCmd::Previous))
                            ),
                            "Página anterior"
                        ),
                        tip(
                            control_seg(
                                t,
                                button(kiri::ori!("chevron-right"))
                                    .on_press(Message::Nav(NavCmd::Next))
                            ),
                            "Próxima página"
                        ),
                    ]
                    .spacing(0)
                    .align_y(Alignment::Center)
                )
                .padding(2)
                .style(kiri::seg_style(t)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding {
            top: 4.0,
            right: 6.0,
            bottom: 4.0,
            left: 12.0,
        })
        .style(kiri::pill_style(t))
        .max_width(520.0);
        let current = match ready.zoom {
            Zoom::Manual(z) => z.get(),
            Zoom::Width | Zoom::Page => ready
                .zoom
                .scale(ready.viewport(), ready.media(ready.visible))
                .factor(),
        };
        let out = Zoom::Manual(ZoomFactor::new(current / 1.1));
        let into = Zoom::Manual(ZoomFactor::new(current * 1.1));

        let zoom_seg = container(
            row![
                tip(
                    control_seg(
                        t,
                        button(kiri::ori!("minus")).on_press(Message::SetZoom(out))
                    ),
                    "Diminuir zoom"
                ),
                tip(
                    control_seg(
                        t,
                        button(kiri::ori!("fit-width")).on_press(Message::SetZoom(Zoom::Width))
                    ),
                    "Ajustar à largura"
                ),
                tip(
                    control_seg(
                        t,
                        button(kiri::ori!("plus")).on_press(Message::SetZoom(into))
                    ),
                    "Aumentar zoom"
                ),
            ]
            .spacing(0)
            .align_y(Alignment::Center),
        )
        .padding(2)
        .style(kiri::seg_style(t));

        // Copiar/fechar/fit-page vivem no menu ⋯ (`overflow_menu`).
        let shield = control_active(
            t,
            button(kiri::ori!("shield")).on_press(Message::ToggleSignatures),
            ready.signatures_open,
        );
        let dot = kiri::status_dot_color(t, ready.signatures.signatures.iter().map(|s| s.status));
        let shield_el: Element<'_, Message> = match dot {
            Some(dot) => tip(
                stack![
                    shield,
                    container(
                        container(Space::with_width(Length::Fixed(7.0)))
                            .width(Length::Fixed(7.0))
                            .height(Length::Fixed(7.0))
                            .style(move |_| container::Style {
                                background: Some(Background::Color(dot)),
                                border: Border {
                                    color: t.chrome,
                                    width: 2.0,
                                    radius: 99.0.into(),
                                },
                                ..container::Style::default()
                            })
                    )
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding(Padding {
                        top: 6.0,
                        right: 6.0,
                        bottom: 0.0,
                        left: 0.0,
                    })
                ],
                "Assinaturas",
            ),
            None => tip(shield, "Assinaturas"),
        };
        let mut right = row![
            tip(
                control_active(
                    t,
                    button(kiri::ori!("pages")).on_press(Message::TogglePages),
                    ready.pages_open
                ),
                "Páginas"
            ),
            shield_el,
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        right = right.push(tip(
            control_active(
                t,
                button(kiri::ori!("more")).on_press(Message::ToggleOverflow),
                ready.overflow_open,
            ),
            "Mais opções",
        ));

        return toolbar_frame(
            t,
            row![
                home_button(t),
                open_button(t),
                kiri::vsep(t),
                Space::with_width(Length::Fill),
                pill,
                Space::with_width(Length::Fill),
                kiri::vsep(t),
                zoom_seg,
                kiri::vsep(t),
                right,
            ]
            .spacing(4)
            .align_y(Alignment::Center)
            .into(),
        );
    }

    // Tela inicial não precisa de home; erro/carregando usam para voltar.
    let mut items: Vec<Element<'_, Message>> = Vec::new();
    if !matches!(session, Session::Empty(_)) {
        items.push(home_button(t));
    }
    items.push(open_button(t));
    toolbar_frame(t, row(items).spacing(4).align_y(Alignment::Center).into())
}

/// Camada do menu ⋯: ocupa tudo mas só os botões capturam clique.
fn overflow_layer(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    container(overflow_menu(ready, t))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::End)
        .align_y(Alignment::Start)
        .padding(Padding {
            top: 44.0,
            right: 8.0,
            bottom: 0.0,
            left: 0.0,
        })
        .into()
}

/// Menu ⋯ (PR 4): zoom página, copiar, fechar, aparência.
fn overflow_menu(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    let mut items = column![].spacing(2).width(Length::Fill);
    items = items.push(menu_item(
        t,
        "Ajustar página inteira",
        Message::SetZoom(Zoom::Page),
    ));
    items = items.push(menu_item(t, "Girar vista (90°)", Message::RotateView));
    items = items.push(print_menu_item(t));
    if ready.can_history_back() {
        items = items.push(menu_item(t, "Voltar", Message::HistoryBack));
    }
    if ready.can_history_forward() {
        items = items.push(menu_item(t, "Avançar", Message::HistoryForward));
    }
    if ready.selection_plain_text().is_some() {
        items = items.push(menu_item(t, "Copiar seleção", Message::CopySelection));
    }
    items = items.push(menu_item(t, "Fechar documento", Message::Close));
    items = items.push(
        container(Space::with_height(Length::Fixed(1.0)))
            .width(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Background::Color(t.line)),
                ..container::Style::default()
            }),
    );
    items = items.push(text("Aparência").size(12).color(t.muted));
    let dark = ready.theme.is_dark();
    items = items.push(
        row![
            menu_theme_button(t, "Escuro", Theme::Dark, dark),
            menu_theme_button(t, "Claro", Theme::Light, !dark),
        ]
        .spacing(4),
    );
    container(items)
        .width(Length::Fixed(232.0))
        .padding(6)
        .style(kiri::menu_style(t))
        .into()
}

fn menu_item(t: Tokens, label: &'static str, message: Message) -> Element<'static, Message> {
    button(text(label).size(13))
        .width(Length::Fill)
        .padding(Padding::from([8, 10]))
        .style(kiri::menu_item_style(t))
        .on_press(message)
        .into()
}

/// ⋯ → Imprimir: abre o diálogo próprio.
fn print_menu_item(t: Tokens) -> Element<'static, Message> {
    button(
        row![kiri::ori!("print"), text("Imprimir").size(13)]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8, 10]))
    .style(kiri::menu_item_style(t))
    .on_press(Message::OpenPrintDialog)
    .into()
}

/// Modal de impressão: fundo fecha ao clicar, cartão captura sem efeito —
/// nada atravessa para o documento atrás.
fn print_layer<'a>(ready: &'a Ready, dialog: &'a PrintDialog, t: Tokens) -> Element<'a, Message> {
    let dim = container(Space::with_width(Length::Fill))
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::from_rgba(0.0, 0.0, 0.0, 0.55))),
            ..container::Style::default()
        });
    let card = container(mouse_area(print_card(ready, dialog, t)).on_press(Message::PrintNop))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);
    stack![mouse_area(dim).on_press(Message::ClosePrintDialog), card,].into()
}

fn print_card<'a>(ready: &'a Ready, dialog: &'a PrintDialog, t: Tokens) -> Element<'a, Message> {
    let pages = dialog.preview_pages(ready.page_count(), ready.visible);
    let at = dialog.preview.min(pages.len().saturating_sub(1));
    let mut col = column![
        row![
            text("Imprimir").size(16),
            Space::with_width(Length::Fill),
            button(text("Fechar").size(13))
                .style(kiri::menu_item_style(t))
                .on_press_maybe((!dialog.busy).then_some(Message::ClosePrintDialog)),
        ]
        .align_y(Alignment::Center),
        row![
            print_preview(ready, dialog, &pages, at, t),
            print_controls(dialog, t),
        ]
        .spacing(16),
    ]
    .spacing(12);
    if let Some(err) = &dialog.error {
        col = col.push(
            text(format!("Não foi possível imprimir: {err}"))
                .size(13)
                .color(t.danger),
        );
    }
    col = col.push(print_footer(dialog, t));
    container(col)
        .width(Length::Fixed(620.0))
        .padding(16)
        .style(kiri::menu_style(t))
        .into()
}

/// Preview reaproveita o thumb do cache (escala de tela, sem render novo).
fn print_preview<'a>(
    ready: &'a Ready,
    dialog: &'a PrintDialog,
    pages: &[PageNo],
    at: usize,
    t: Tokens,
) -> Element<'a, Message> {
    let thumb: Element<'a, Message> =
        match pages.get(at).and_then(|page| ready.thumb_surface(*page)) {
            Some(surface) => image(surface.image.clone())
                .width(Length::Fixed(220.0))
                .into(),
            None => container(text("carregando…").size(13).color(t.muted))
                .width(Length::Fixed(220.0))
                .height(Length::Fixed(280.0))
                .align_x(Alignment::Center)
                .align_y(Alignment::Center)
                .into(),
        };
    let pager = row![
        button(kiri::ori!("chevron-left"))
            .style(kiri::ibtn_style(t, false))
            .on_press_maybe((!dialog.busy && at > 0).then_some(Message::PrintPreviewPrev)),
        text(format!("{} de {}", at + 1, pages.len().max(1))).size(13),
        button(kiri::ori!("chevron-right"))
            .style(kiri::ibtn_style(t, false))
            .on_press_maybe(
                (!dialog.busy && at + 1 < pages.len()).then_some(Message::PrintPreviewNext)
            ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![thumb, pager]
        .spacing(8)
        .align_x(Alignment::Center)
        .into()
}

fn print_controls(dialog: &PrintDialog, t: Tokens) -> Element<'_, Message> {
    let busy = dialog.busy;
    let printer: Element<'_, Message> = if dialog.printers_loading {
        text("Carregando impressoras…")
            .size(13)
            .color(t.muted)
            .into()
    } else if dialog.printers.is_empty() {
        text("Nenhuma impressora encontrada")
            .size(13)
            .color(t.warn)
            .into()
    } else {
        let names: Vec<String> = dialog.printers.iter().map(|p| p.name.clone()).collect();
        let selected = dialog.selected_printer().map(|p| p.name.clone());
        pick_list(names.clone(), selected, move |name: String| {
            Message::PrintSelectPrinter(names.iter().position(|n| *n == name).unwrap_or(0))
        })
        .placeholder("Impressora")
        .width(Length::Fill)
        .into()
    };
    let mut col = column![
        section_title("Impressora", t),
        printer,
        section_title("Páginas", t),
        row![
            seg_button(t, "Todas", RangeMode::All, dialog, busy),
            seg_button(t, "Atual", RangeMode::Current, dialog, busy),
            seg_button(t, "De–Até", RangeMode::Custom, dialog, busy),
        ]
        .spacing(4),
    ]
    .spacing(6);
    if dialog.range_mode == RangeMode::Custom {
        col = col.push(
            row![
                text_input("De", &dialog.from_input)
                    .on_input(Message::PrintSetFromInput)
                    .width(Length::Fixed(64.0)),
                text("até").size(13).color(t.muted),
                text_input("Até", &dialog.to_input)
                    .on_input(Message::PrintSetToInput)
                    .width(Length::Fixed(64.0)),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }
    col = col.push(section_title("Cópias", t));
    col = col.push(
        row![
            button(kiri::ori!("minus"))
                .style(kiri::ibtn_style(t, false))
                .on_press_maybe((!busy && dialog.copies > 1).then_some(Message::PrintCopiesMinus)),
            container(text(dialog.copies.to_string()).size(14))
                .width(Length::Fixed(32.0))
                .align_x(Alignment::Center),
            button(kiri::ori!("plus"))
                .style(kiri::ibtn_style(t, false))
                .on_press_maybe(
                    (!busy && dialog.copies < MAX_COPIES).then_some(Message::PrintCopiesPlus)
                ),
        ]
        .spacing(4)
        .align_y(Alignment::Center),
    );
    col = col.push(section_title("Orientação", t));
    col = col.push(
        row![
            ori_button(t, "Automática", PrintOrientation::Auto, dialog, busy),
            ori_button(t, "Retrato", PrintOrientation::Portrait, dialog, busy),
            ori_button(t, "Paisagem", PrintOrientation::Landscape, dialog, busy),
        ]
        .spacing(4),
    );
    col.width(Length::Fill).into()
}

fn seg_button(
    t: Tokens,
    label: &'static str,
    mode: RangeMode,
    dialog: &PrintDialog,
    busy: bool,
) -> Element<'static, Message> {
    button(text(label).size(13))
        .style(kiri::ibtn_style(t, dialog.range_mode == mode))
        .on_press_maybe((!busy).then_some(Message::PrintSetRangeMode(mode)))
        .into()
}

fn ori_button(
    t: Tokens,
    label: &'static str,
    orientation: PrintOrientation,
    dialog: &PrintDialog,
    busy: bool,
) -> Element<'static, Message> {
    button(text(label).size(13))
        .style(kiri::ibtn_style(t, dialog.orientation == orientation))
        .on_press_maybe((!busy).then_some(Message::PrintSetOrientation(orientation)))
        .into()
}

fn print_footer(dialog: &PrintDialog, t: Tokens) -> Element<'static, Message> {
    let busy = dialog.busy;
    let label = if busy { "Enviando…" } else { "Imprimir" };
    let secondary = |label: &'static str, message: Message| {
        button(text(label).size(13))
            .padding(Padding::from([8, 12]))
            .style(kiri::menu_item_style(t))
            .on_press_maybe((!busy).then_some(message))
    };
    row![
        Space::with_width(Length::Fill),
        secondary("Abrir PDF", Message::PrintOpenPdf),
        secondary("Cancelar", Message::ClosePrintDialog),
        button(text(label).size(13))
            .padding(Padding::from([8, 12]))
            .style(kiri::menu_item_style(t))
            .on_press_maybe((!busy).then_some(Message::PrintSubmit)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

fn menu_theme_button(
    t: Tokens,
    label: &'static str,
    theme: Theme,
    active: bool,
) -> Element<'static, Message> {
    control_active(
        t,
        button(text(label).size(13)).on_press(Message::SetTheme(theme)),
        active,
    )
    .width(Length::Fill)
    .into()
}

/// Progresso Kiri: 2px, fill `accent` proporcional à página visível.
fn progress(ready: &Ready, t: Tokens) -> Element<'static, Message> {
    let n = ready.page_count().max(1);
    let done = (ready.visible.index() + 1).min(n);
    // Escala fixa em 1000 partes — sem overflow de `FillPortion` em docs grandes.
    let filled = ((done * 1000) / n).clamp(1, 1000) as u16;
    let mut bar = row![container(Space::with_width(Length::Fill))
        .width(Length::FillPortion(filled))
        .height(Length::Fixed(2.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(t.accent)),
            ..container::Style::default()
        }),]
    .spacing(0);
    if filled < 1000 {
        bar = bar.push(Space::new(
            Length::FillPortion(1000 - filled),
            Length::Shrink,
        ));
    }
    container(bar)
        .width(Length::Fill)
        .height(Length::Fixed(2.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(t.surface)),
            ..container::Style::default()
        })
        .into()
}

fn empty_browser(empty: &EmptyState, t: Tokens) -> Element<'_, Message> {
    let mut path_row = row![].spacing(6).align_y(Alignment::Center);
    if let Some(parent) = empty.parent() {
        path_row = path_row.push(tip(
            control(
                t,
                button(icon!(t, "chevron-left")).on_press(Message::BrowseTo(parent)),
            ),
            "Voltar",
        ));
    }
    path_row = path_row.push(text(empty.path_label()).size(14));

    let mut listing = column![].spacing(4);
    if let Some(err) = &empty.listing_error {
        listing = listing.push(text(err).size(13));
    } else if empty.listing.is_empty() {
        listing = listing.push(text("Nenhuma pasta ou PDF aqui.").size(13));
    } else {
        for entry in &empty.listing {
            listing = listing.push(entry_row(entry, t));
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
                container(
                    control(
                        t,
                        button(
                            row![icon!(t, "file-text"), text(name).size(14)]
                                .spacing(8)
                                .align_y(Alignment::Center),
                        )
                        .width(Length::Fill)
                        .on_press(Message::OpenRecent(path.clone())),
                    )
                    .width(Length::Fill),
                )
                .width(Length::Fill)
                .padding(12)
                .style(kiri::recent_card_style(t)),
            );
        }
    }

    column![
        container(
            column![
                image(image::Handle::from_bytes(
                    &include_bytes!("../../../public/tsuro-horizontal.png")[..],
                ))
                .width(Length::Fixed(200.0)),
                text("Abra um PDF ou arraste para cá")
                    .size(14)
                    .color(t.muted),
            ]
            .spacing(8)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding::from([16, 0])),
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
    .height(Length::Fill)
    .into()
}

fn entry_row(entry: &FsEntry, t: Tokens) -> Element<'static, Message> {
    let message = if entry.is_dir {
        Message::BrowseTo(Some(entry.path.clone()))
    } else {
        Message::OpenRecent(entry.path.clone())
    };
    let glyph: Element<'static, Message> = if entry.is_dir {
        icon!(t, "folder").into()
    } else {
        icon!(t, "file-text").into()
    };
    control(
        t,
        button(
            row![glyph, text(entry.name.clone()).size(14)]
                .spacing(8)
                .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .on_press(message),
    )
    .width(Length::Fill)
    .into()
}

fn ready_body(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    let mut panes = row![].spacing(12).height(Length::Fill);
    if ready.pages_open {
        panes = panes.push(pages_panel(ready, t));
    }
    panes = panes.push(page_pane(ready, t));
    if ready.signatures_open {
        panes = panes.push(signatures_panel(ready, t));
    }
    // Status pós-envio ("Enviado para …"): 1 linha no topo do corpo.
    if let Some(status) = &ready.print_status {
        column![text(status).size(13).color(t.muted), panes,]
            .spacing(8)
            .height(Length::Fill)
            .into()
    } else {
        panes.into()
    }
}

fn pages_panel(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    // Janela virtualizada do remoto: só monta as miniaturas visíveis.
    let window = ready.thumb_page_window();
    let start = window.first().map(|page| page.index()).unwrap_or(0);
    let end = window.last().map(|page| page.index() + 1).unwrap_or(0);
    let mut col = column![section_title("Páginas", t)].spacing(8);
    if start > 0 {
        col = col.push(Space::with_height(Length::Fixed(start as f32 * THUMB_ROW)));
    }
    for i in start..end {
        let page = PageNo::from_index(i);
        let preview: Element<'_, Message> = match ready.thumb_surface(page) {
            Some(surface) => image(surface.image.clone())
                .width(Length::Fixed(120.0))
                .into(),
            None => container(text("…").size(13))
                .width(Length::Fixed(120.0))
                .height(Length::Fixed(150.0))
                .center_x(Length::Fill)
                .center_y(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(t.elevated)),
                    border: Border {
                        color: t.line,
                        width: 1.0,
                        radius: 4.0.into(),
                    },
                    text_color: Some(t.ink),
                    ..container::Style::default()
                })
                .into(),
        };
        let active = ready.visible == page;
        col = col.push(
            control_active(
                t,
                button(
                    column![
                        // Borda 2px sempre (transparente fora da ativa) — sem shift de layout.
                        container(preview)
                            .padding(0)
                            .style(move |_| container::Style {
                                border: Border {
                                    color: if active { t.accent } else { Color::TRANSPARENT },
                                    width: 2.0,
                                    radius: 4.0.into(),
                                },
                                ..container::Style::default()
                            }),
                        text(format!("{}", i + 1)).size(12)
                    ]
                    .spacing(4)
                    .align_x(Alignment::Center),
                )
                .on_press(Message::Nav(NavCmd::GoTo(page))),
                active,
            )
            .width(Length::Fill),
        );
    }
    let remaining = ready.page_count().saturating_sub(end);
    if remaining > 0 {
        col = col.push(Space::with_height(Length::Fixed(
            remaining as f32 * THUMB_ROW,
        )));
    }
    scrollable(col)
        .id(pages_scroll_id())
        .on_scroll(|viewport| Message::PagesScrolled(viewport.absolute_offset().y))
        .width(Length::Fixed(156.0))
        .height(Length::Fill)
        .into()
}

fn page_pane(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    let page_view: Element<'_, Message> = match ready.visible_surface() {
        Some(surface) => image(surface.image.clone()).width(Length::Fill).into(),
        None if ready.visible_render_failed() => {
            text("Não foi possível renderizar esta página.").into()
        }
        None => text("Renderizando página…").into(),
    };

    scrollable(
        container(
            container(page_view)
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(0)
                .style(kiri::page_frame(&t)),
        )
        .width(Length::Fill)
        .center_x(Length::Fill)
        .padding(Padding {
            top: 28.0,
            right: 24.0,
            bottom: 32.0,
            left: 24.0,
        })
        .style(move |_| container::Style {
            background: Some(Background::Color(t.surface)),
            ..container::Style::default()
        }),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn signatures_panel(ready: &Ready, t: Tokens) -> Element<'_, Message> {
    let mut col = column![section_title("Assinaturas", t)].spacing(8);
    if ready.signatures.signatures.is_empty() {
        col = col.push(
            text("Nenhuma assinatura neste arquivo.")
                .size(13)
                .color(t.muted),
        );
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
            let dot = kiri::status_color(t, sig.status);
            col = col.push(
                container(
                    column![
                        row![status_dot(dot), text(name).size(14).color(t.ink),]
                            .spacing(8)
                            .align_y(Alignment::Center),
                        text(status_label(sig.status)).size(13).color(t.ink),
                        text(&sig.status_detail).size(12).color(t.muted),
                    ]
                    .spacing(4),
                )
                .width(Length::Fill)
                .padding(12)
                .style(move |_| container::Style {
                    background: Some(Background::Color(t.elevated)),
                    border: Border {
                        color: t.line,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    text_color: Some(t.ink),
                    ..container::Style::default()
                }),
            );
        }
    }
    container(scrollable(col).height(Length::Fill))
        .width(Length::Fixed(220.0))
        .height(Length::Fill)
        .padding(Padding::from([4, 0]))
        .style(move |_| container::Style {
            background: Some(Background::Color(t.bg)),
            ..container::Style::default()
        })
        .into()
}

/// Título de seção de painel: 12px, maiúsculas, `muted`.
fn section_title(label: &'static str, t: Tokens) -> Element<'static, Message> {
    text(label.to_uppercase()).size(12).color(t.muted).into()
}

/// Dot 8px de status (cartões de assinatura).
fn status_dot(color: Color) -> Element<'static, Message> {
    container(Space::with_width(Length::Fixed(8.0)))
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: Border {
                radius: 99.0.into(),
                ..Border::default()
            },
            ..container::Style::default()
        })
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
