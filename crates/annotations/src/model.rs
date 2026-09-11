//! Persisted annotation data and tool descriptions.
use crate::timing::AnnotationTiming;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tool {
    Select,
    Rectangle,
    FilledRectangle,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Number,
    Text,
    Pixelate,
    Blur,
    Highlight,
}

impl Tool {
    pub const ALL: [(Tool, &'static str); 12] = [
        (Tool::Select, "icons/select.svg"),
        (Tool::Rectangle, "icons/rectangle.svg"),
        (Tool::FilledRectangle, "icons/filled-rectangle.svg"),
        (Tool::Ellipse, "icons/ellipse.svg"),
        (Tool::Line, "icons/line.svg"),
        (Tool::Arrow, "icons/arrow.svg"),
        (Tool::Pen, "icons/pen.svg"),
        (Tool::Number, "icons/number.svg"),
        (Tool::Text, "icons/text.svg"),
        (Tool::Pixelate, "icons/pixelate.svg"),
        (Tool::Blur, "icons/blur.svg"),
        (Tool::Highlight, "icons/highlight.svg"),
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rectangle => "Rectangle",
            Tool::FilledRectangle => "Filled rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Pen => "Pen",
            Tool::Number => "Number",
            Tool::Text => "Text",
            Tool::Pixelate => "Pixelate",
            Tool::Blur => "Blur",
            Tool::Highlight => "Highlight",
        }
    }

    pub fn help_text(self) -> &'static str {
        match self {
            Tool::Select => {
                "Click to select, Shift-click to add; drag empty space to select several"
            }
            Tool::Rectangle => "Drag to draw an outlined rectangle",
            Tool::FilledRectangle => "Drag to draw a solid rectangle",
            Tool::Ellipse => "Drag to draw a circle or ellipse",
            Tool::Line => "Drag between two endpoints for a straight line",
            Tool::Arrow => "Drag an arrow; select it to move endpoints or bend its middle",
            Tool::Pen => "Drag to draw smooth ink; click to make a dot",
            Tool::Number => "Click to place the next numbered circle",
            Tool::Text => "Click to type, drag for wrapping text; Ctrl+Enter finishes editing",
            Tool::Pixelate => "Drag over an area to hide it with pixels",
            Tool::Blur => "Drag over an area to obscure it with blur",
            Tool::Highlight => "Drag an area to keep visible; everything outside is dimmed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnnotationMark {
    pub tool: Tool,
    pub start: NormPoint,
    pub end: NormPoint,
    pub points: Vec<NormPoint>,
    /// Signed arc sagitta as a fraction of the endpoint distance.
    pub bend: f32,
    pub text_auto_width: bool,
    #[serde(skip)]
    pub draw_progress: Option<f32>,
    /// Live strokes trail the pointer slightly; release settles the final cap.
    #[serde(skip)]
    pub ink_in_progress: bool,
    /// Original width before a preview/export transform, so zoom does not
    /// change the pressure or streamline settings.
    #[serde(skip)]
    pub ink_style_width: Option<f32>,
    pub number: usize,
    pub color: u32,
    pub stroke_width: f32,
    pub hand_drawn: bool,
    /// Stable variation, retained across moves, undo and export.
    pub draw_seed: u32,
    pub density: f32,
    pub text: String,
    pub font_size: f32,
    pub font_family: u8,
    pub text_alignment: u8,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// When the scene is animated: when and how the mark appears.
    pub timing: Option<AnnotationTiming>,
    /// Painted opacity (animation applies its fade here).
    pub opacity: f32,
    /// Placed by a template; replaced when another template is applied.
    pub from_template: bool,
    /// Anchored to the visible frame instead of the media, so camera motion
    /// pans beneath it (captions, step numbers).
    pub pinned: bool,
    /// Positioned on the scene canvas, independent of media placement and lifetime.
    pub canvas: bool,
}

impl AnnotationMark {
    /// Scale presentation width while retaining the ink's document-space style.
    pub fn scale_stroke_width(&mut self, scale: f32) {
        if self.tool == Tool::Pen {
            self.ink_style_width.get_or_insert(self.stroke_width);
        }
        self.stroke_width *= scale;
    }

    /// Template captions belong to the full scene, including in projects saved
    /// before templates explicitly set `canvas`. Media callouts stay attached.
    pub fn is_canvas(&self) -> bool {
        self.canvas || (self.from_template && self.tool == Tool::Text)
    }
}

impl Default for AnnotationMark {
    fn default() -> Self {
        Self {
            tool: Tool::Rectangle,
            start: NormPoint::default(),
            end: NormPoint::default(),
            points: Vec::new(),
            bend: 0.0,
            text_auto_width: true,
            draw_progress: None,
            ink_in_progress: false,
            ink_style_width: None,
            number: 1,
            color: ANNOTATION_COLORS[1].1,
            stroke_width: 4.0,
            hand_drawn: false,
            draw_seed: 0,
            density: 0.5,
            text: String::new(),
            font_size: 24.0,
            font_family: 0,
            text_alignment: 0,
            bold: false,
            italic: false,
            underline: false,
            timing: None,
            opacity: 1.0,
            from_template: false,
            pinned: false,
            canvas: false,
        }
    }
}

/// Annotations plus their undo history, so the screenshot editor's marks
/// survive a detour through the recording editor (which has its own set).
#[derive(Clone, Debug, Default)]
pub struct AnnotationWorkspace {
    pub marks: Vec<AnnotationMark>,
    pub undo: Vec<Vec<AnnotationMark>>,
    pub redo: Vec<Vec<AnnotationMark>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NormPoint {
    pub x: f32,
    pub y: f32,
}

pub const ANNOTATION_COLORS: [(&str, u32); 10] = [
    ("Black", 0x050506),
    ("Red", 0xf73833),
    ("Orange", 0xff8714),
    ("Yellow", 0xffd12e),
    ("Green", 0x2eb85c),
    ("Turquoise", 0x33c4b8),
    ("Blue", 0x2e7aff),
    ("Purple", 0x8c4cf2),
    ("Pink", 0xff2e6e),
    ("White", 0xf5f5f5),
];
