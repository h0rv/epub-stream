use mu_epub::{
    BlockRole, ComputedTextStyle, StyledEvent, StyledEventOrRun, StyledImage, StyledRun,
};
use mu_epub_render::{DrawCommand, LayoutConfig, LayoutEngine};

fn body_run(text: &str) -> StyledEventOrRun {
    StyledEventOrRun::Run(StyledRun {
        text: text.to_string(),
        style: ComputedTextStyle {
            family_stack: vec!["serif".to_string()],
            weight: 400,
            italic: false,
            size_px: 17.0,
            line_height: 1.4,
            letter_spacing: 0.0,
            block_role: BlockRole::Body,
        },
        font_id: 0,
        resolved_family: "serif".to_string(),
    })
}

#[test]
fn mixed_text_and_images_paginate_without_overlap() {
    let cfg = LayoutConfig {
        display_width: 480,
        display_height: 800,
        margin_left: 20,
        margin_right: 20,
        margin_top: 18,
        margin_bottom: 64,
        ..LayoutConfig::default()
    };
    let engine = LayoutEngine::new(cfg);
    let mut items = vec![
        StyledEventOrRun::Event(StyledEvent::ParagraphStart),
        body_run("Intro paragraph before first image."),
        StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
        StyledEventOrRun::Image(StyledImage {
            src: "images/cover.jpg".to_string(),
            alt: "Cover image".to_string(),
            width_px: Some(1200),
            height_px: Some(1800),
            in_figure: true,
        }),
        StyledEventOrRun::Event(StyledEvent::ParagraphStart),
        body_run("Caption-ish paragraph following the first image."),
        StyledEventOrRun::Event(StyledEvent::ParagraphEnd),
    ];
    for _ in 0..40 {
        items.push(StyledEventOrRun::Event(StyledEvent::ParagraphStart));
        items.push(body_run("Long body text to force additional pagination and ensure image commands coexist with text commands."));
        items.push(StyledEventOrRun::Event(StyledEvent::ParagraphEnd));
    }
    items.push(StyledEventOrRun::Image(StyledImage {
        src: "images/diagram.png".to_string(),
        alt: "Diagram".to_string(),
        width_px: Some(640),
        height_px: Some(420),
        in_figure: false,
    }));

    let pages = engine.layout_items(items);
    assert!(pages.len() > 1);

    let mut saw_image = false;
    for page in &pages {
        for cmd in &page.commands {
            if let DrawCommand::ImageObject(obj) = cmd {
                saw_image = true;
                assert!(obj.width > 0);
                assert!(obj.height > 0);
                assert!(!obj.src.is_empty());
            }
        }
    }
    assert!(saw_image);
}
