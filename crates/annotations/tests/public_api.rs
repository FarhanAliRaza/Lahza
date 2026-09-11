use gpui::{point, px, size, Bounds};
use lahza_annotations::{canvas, svg, timing, AnnotationMark, AnnotationTiming, NormPoint, Tool};

#[test]
fn persisted_marks_can_be_edited_animated_and_exported_through_the_public_api() {
    // This is the pre-extraction document schema, consumed as a downstream crate.
    let document = r##"{"tool":"arrow","start":{"x":0.1,"y":0.2},"end":{"x":0.8,"y":0.6},"bend":0.25,"handDrawn":true,"drawSeed":42,"color":16201779,"timing":{"start":1.0,"end":3.5,"entrance":"draw","exit":"fade","transition":0.35}}"##;
    let mut mark: AnnotationMark = serde_json::from_str(document).unwrap();
    assert_eq!(mark.tool, Tool::Arrow);
    assert!(mark.text_auto_width);
    assert_eq!(mark.draw_progress, None);
    canvas::translate_mark(&mut mark, 0.05, 0.0);
    let saved = serde_json::to_value(&mark).unwrap();
    assert_eq!(saved["handDrawn"], true);
    assert_eq!(saved["drawSeed"], 42);
    assert!(saved.get("drawProgress").is_none());
    assert_eq!(
        serde_json::from_value::<AnnotationMark>(saved).unwrap(),
        mark
    );

    assert!(timing::animated_mark(&mark, 0.0).is_none());
    let visible = timing::animated_mark(&mark, mark.timing.unwrap().editing_time(1.0)).unwrap();
    let layer = svg::render_annotations(&[visible], 800, 600).unwrap();
    assert!(layer.pixels().any(|p| p[3] != 0));
    let bounds = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(600.)));
    assert_eq!(
        canvas::norm_to_screen(NormPoint { x: 0.5, y: 0.5 }, bounds),
        point(px(400.), px(300.))
    );
    assert!(AnnotationTiming::for_tool(Tool::Text, 5.0, 5.0).editing_time(5.0) < 5.0);
}
