use std::ops::ControlFlow;

use comfy::{anyhow::Context, image::GenericImageView, *};
use serde::Deserialize;

use crate::{assets_dir, Direction};

#[derive(Deserialize)]
struct AnimationParams {
    sprite_sheet: SpriteSheetParams,
    sprites: Vec<AnnotatedSpriteParams>,
    looping: bool,
    #[serde(default)]
    play_backwards: bool,
}

#[derive(Deserialize)]
struct AnnotatedSpriteParams {
    hurtbox: bool,
    /// Pixel coords in the current tile of the spritesheet.
    /// Top-left is (0,0).
    hitbox: Option<PixelRect>,
    duration: usize,
    sprite_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct PixelRect {
    offset: [u32; 2],
    size: [u32; 2],
}

impl From<PixelRect> for IRect {
    fn from(r: PixelRect) -> Self {
        IRect {
            offset: IVec2 {
                x: r.offset[0] as i32,
                y: r.offset[1] as i32,
            },
            size: IVec2 {
                x: r.size[0] as i32,
                y: r.size[1] as i32,
            },
        }
    }
}

#[derive(Deserialize)]
struct SpriteSheetParams {
    texture: String,
    count_x: u32,
    count_y: u32,
}

#[derive(Clone)]
pub struct Animation {
    data: Rc<AnimationData>,
    sprite_index: usize,
    frame_counter: usize,
}

pub struct AnimationData {
    sprites: Vec<AnnotatedSprite>,
    looping: bool,
}

/// Using player frame of refence, world scale.
pub struct AnnotatedSprite {
    texture: TextureHandle,
    source_rect: PixelRect,
    pub hitbox: Option<AABB>,
    pub hurtbox: Option<AABB>,
    size: Vec2,
    duration: usize,
}

pub fn load_animations() -> HashMap<String, Rc<AnimationData>> {
    let mut anims = HashMap::new();

    for file in assets_dir().files() {
        let path = file.path();
        let Some(ext) = path.extension() else {
            continue;
        };
        if ext == "ron" {
            let name = path.file_stem().unwrap().to_str().unwrap().to_string();
            let contents = file.contents_utf8().unwrap();
            let data = Rc::new(load_animation(
                ron::from_str::<AnimationParams>(contents)
                    .with_context(|| name.clone())
                    .unwrap_or_else(|e| panic!("{e}")),
            ));
            anims.insert(name, data);
        }
    }

    anims
}

impl AnimationData {
    pub fn to_anim(self: &Rc<Self>) -> Animation {
        Animation {
            data: Rc::clone(self),
            sprite_index: 0,
            frame_counter: 0,
        }
    }
}

impl Animation {
    pub fn is_instance(&self, data: &Rc<AnimationData>) -> bool {
        Rc::ptr_eq(&self.data, data)
    }
}

fn load_animation(anim: AnimationParams) -> AnimationData {
    let mut sprites = Vec::with_capacity(anim.sprites.len());

    let mut expected_sprite_index = 0;
    for params in anim.sprites {
        let sprite_index = params.sprite_index.unwrap_or(expected_sprite_index);
        sprites.push(load_sprite(sprite_index, &anim.sprite_sheet, &params));
        expected_sprite_index = sprite_index + 1;
    }

    if anim.play_backwards {
        sprites.reverse();
    }
    AnimationData {
        looping: anim.looping,
        sprites,
    }
}

fn load_sprite(
    sprite_index: usize,
    sprite_sheet: &SpriteSheetParams,
    sprite: &AnnotatedSpriteParams,
) -> AnnotatedSprite {
    let texture = texture_id(&sprite_sheet.texture);
    let assets_lock = ASSETS.borrow();
    let images_lock = assets_lock.texture_image_map.lock();
    let image = images_lock.get(&texture).unwrap();

    let sprite_width = image.width() / sprite_sheet.count_x;
    let sprite_height = image.height() / sprite_sheet.count_y;

    let x = sprite_index as u32 % sprite_sheet.count_x;
    let y = sprite_index as u32 / sprite_sheet.count_x;
    assert!(y < sprite_sheet.count_y);

    let sprite_x = sprite_width * x;
    let sprite_y = sprite_height * y;

    let sprite_image =
        comfy::image::imageops::crop_imm(image, sprite_x, sprite_y, sprite_width, sprite_height);

    let mut min_x = sprite_width - 1;
    let mut max_x = 0; // exclusive
    for (x, _y, value) in sprite_image.pixels() {
        if value.0[3] != 0 {
            min_x = min_x.min(x);
            max_x = max_x.max(x + 1);
        }
    }

    let mut min_y = sprite_height - 1;
    let mut max_y = 0; // exclusive
    for (_x, y, value) in sprite_image.pixels() {
        if value.0[3] != 0 {
            min_y = min_y.min(y);
            max_y = max_y.max(y + 1);
        }
    }

    let normalization_factor = sprite_width as f32 * 2.5;

    let hurtbox = AABB {
        min: Vec2 {
            x: (min_x as f32 - sprite_width as f32 / 2.) / normalization_factor,
            y: -1. * (max_y as f32 - sprite_height as f32 / 2.) / normalization_factor,
        },
        max: Vec2 {
            x: (max_x as f32 - sprite_width as f32 / 2.) / normalization_factor,
            y: -1. * (min_y as f32 - sprite_height as f32 / 2.) / normalization_factor,
        },
    };

    let hitbox = sprite.hitbox.map(|rect| {
        // in: pixel, y-down, (0,0) topleft
        // out: float, y-up, (0,0) center of player

        let center_x = sprite_width as f32 / 2.;
        let center_y = sprite_height as f32 / 2.;
        let x = (rect.offset[0] as f32 - center_x) / normalization_factor;
        let y = -1. * (rect.offset[1] as f32 - center_y) / normalization_factor;

        let w = rect.size[0] as f32 / normalization_factor;
        let h = rect.size[1] as f32 / normalization_factor;

        AABB::from_top_left(Vec2 { x, y }, Vec2 { x: w, y: h })
    });

    AnnotatedSprite {
        texture,
        source_rect: PixelRect {
            offset: [sprite_x, sprite_y],
            size: [sprite_width, sprite_height],
        },
        hurtbox: sprite.hurtbox.then(|| hurtbox),
        hitbox,
        size: Vec2 {
            x: sprite_width as f32 / normalization_factor,
            y: sprite_height as f32 / normalization_factor,
        },
        duration: sprite.duration,
    }
}

impl Animation {
    pub fn next_frame(&mut self) -> ControlFlow<()> {
        self.frame_counter += 1;
        if self.data.sprites[self.sprite_index].duration <= self.frame_counter {
            self.sprite_index += 1;
            self.frame_counter = 0;

            if self.sprite_index >= self.data.sprites.len() {
                self.sprite_index = 0;
                if !self.data.looping {
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    }

    pub fn sprite(&self) -> &AnnotatedSprite {
        &self.data.sprites[self.sprite_index]
    }

    /// Player 1 faces right (no flip); player 2 faces left (flip).
    pub fn render(&self, tint: Color, location: Vec2, facing: Direction) {
        let sprite = self.sprite();
        draw_sprite_ex(
            sprite.texture,
            location,
            tint,
            2,
            DrawTextureParams {
                dest_size: Some(sprite.size.as_world_size()),
                source_rect: Some(sprite.source_rect.into()),
                scroll_offset: Vec2::ZERO,
                rotation: 0.,
                flip_x: matches!(facing, Direction::West),
                flip_y: false,
                pivot: None,
                blend_mode: BlendMode::Alpha,
            },
        );
    }
}
