pub const SOLID_BACKGROUNDS: [(&str, u32); 16] = [
    ("Black", 0x050506),
    ("White", 0xf5f5f0),
    ("Graphite", 0x2b2e36),
    ("Red", 0xf03b47),
    ("Orange", 0xf78429),
    ("Yellow", 0xf5ba3b),
    ("Green", 0x3b9c5c),
    ("Blue", 0x2980e0),
    ("Purple", 0x7a42e8),
    ("Blush", 0xeda89e),
    ("Mint", 0xa8e6ba),
    ("Sky", 0xa1c9f0),
    ("Lavender", 0xccc2eb),
    ("Peach", 0xfaccb0),
    ("Sage", 0xbdd1b3),
    ("Sand", 0xe8dec2),
];

#[derive(Clone, Copy)]
pub struct GradientPreset {
    pub title: &'static str,
    pub colors: [u32; 3],
    pub angle: f32,
}

pub const GRADIENT_BACKGROUNDS: [GradientPreset; 16] = [
    GradientPreset {
        title: "Aurora",
        colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
        angle: 135.0,
    },
    GradientPreset {
        title: "Cobalt",
        colors: [0x0a0d80, 0x4230ed, 0x6babfa],
        angle: 160.0,
    },
    GradientPreset {
        title: "Peach",
        colors: [0xfa615c, 0xfcb55c, 0xe654a6],
        angle: 135.0,
    },
    GradientPreset {
        title: "Glass",
        colors: [0xdef2f0, 0x75c4db, 0x4087ed],
        angle: 135.0,
    },
    GradientPreset {
        title: "Plasma",
        colors: [0x140538, 0x591fd6, 0xf2426b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Mango",
        colors: [0xfcbf33, 0xf55436, 0xab30e3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mist",
        colors: [0xf0f0eb, 0xcce0f0, 0xf2c2b3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Lagoon",
        colors: [0x144d8a, 0x40a3b8, 0xb3ebc7],
        angle: 45.0,
    },
    GradientPreset {
        title: "Ember",
        colors: [0x2e0814, 0xdb2b2e, 0xffab40],
        angle: 135.0,
    },
    GradientPreset {
        title: "Violet",
        colors: [0x3d1482, 0x9638f0, 0xf56bbd],
        angle: 160.0,
    },
    GradientPreset {
        title: "Sea Glass",
        colors: [0x6edbbf, 0x409ecc, 0x3859bf],
        angle: 135.0,
    },
    GradientPreset {
        title: "Citrus",
        colors: [0xfce84d, 0x70c74a, 0x1f946b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Amethyst",
        colors: [0x1a1447, 0x5926a6, 0xc263f2],
        angle: 45.0,
    },
    GradientPreset {
        title: "Sorbet",
        colors: [0xff7d82, 0xffbd7a, 0x8fc7fa],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mineral",
        colors: [0xedf5f2, 0xa3b8d1, 0x546b8c],
        angle: 135.0,
    },
    GradientPreset {
        title: "Dawn",
        colors: [0xfa9ec4, 0xfad178, 0x6bb5f5],
        angle: 45.0,
    },
];
