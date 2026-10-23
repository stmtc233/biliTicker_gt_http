use crate::error::{other, other_without_source, parse_error, Result};
use image::imageops::{resize, FilterType};
use image::{DynamicImage, GrayImage, Luma, Rgb, RgbImage, RgbaImage};
use ndarray::Array4;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const DETECT_SIZE: u32 = 320;
const DETECT_CONF: f32 = 0.05;
const DETECT_IOU: f32 = 0.45;
const MAX_CANDIDATES: usize = 14;
const SIAMESE_SIZE: u32 = 64;
const HOG_SIZE: u32 = 64;
const SIM_RANGE: (f32, f32) = (0.03, 0.96);
const EDGE_HOG_RANGE: (f32, f32) = (0.58, 0.86);
const LOCAL_RANGE: (f32, f32) = (0.66, 0.89);
const SIM_WEIGHT: f32 = 0.10;
const EDGE_HOG_WEIGHT: f32 = 0.90;
const LOCAL_WEIGHT: f32 = 0.40;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Candidate {
    pub x1: u32,
    pub y1: u32,
    pub x2: u32,
    pub y2: u32,
    pub confidence: f32,
}

impl Candidate {
    fn width(self) -> u32 {
        self.x2.saturating_sub(self.x1)
    }

    fn height(self) -> u32 {
        self.y2.saturating_sub(self.y1)
    }

    fn center(self) -> (f64, f64) {
        (
            (self.x1 + self.x2) as f64 / 2.0,
            (self.y1 + self.y2) as f64 / 2.0,
        )
    }
}

#[derive(Debug, Clone)]
pub struct MatchPlan {
    pub clicks: Vec<Vec<(f64, f64)>>,
    pub candidate_count: usize,
    pub phase: &'static str,
}

struct SessionPool {
    sessions: Vec<Mutex<Session>>,
    cursor: AtomicUsize,
}

impl SessionPool {
    fn load(path: &Path, copies: usize, threads: usize) -> Result<Self> {
        if !path.is_file() {
            return Err(other_without_source(&format!(
                "ONNX model not found: {}",
                path.display()
            )));
        }
        let mut sessions = Vec::with_capacity(copies.max(1));
        for _ in 0..copies.max(1) {
            let session = Session::builder()
                .map_err(|e| other("create ONNX session builder failed", e))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| other("set ONNX optimization level failed", e))?
                .with_intra_threads(threads.max(1))
                .map_err(|e| other("set ONNX intra-op threads failed", e))?
                .with_inter_threads(1)
                .map_err(|e| other("set ONNX inter-op threads failed", e))?
                .commit_from_file(path)
                .map_err(|e| other("load ONNX model failed", e))?;
            sessions.push(Mutex::new(session));
        }
        Ok(Self {
            sessions,
            cursor: AtomicUsize::new(0),
        })
    }

    fn next(&self) -> &Mutex<Session> {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed) % self.sessions.len();
        &self.sessions[index]
    }
}

pub struct IconRecognizer {
    detector: SessionPool,
    fallback_detector: Option<SessionPool>,
    matcher: SessionPool,
}

struct QuestionAsset {
    siamese: Array4<f32>,
    edge_hogs: Vec<Vec<f32>>,
    rotated_masks: Vec<GrayImage>,
}

struct CandidateAsset {
    crop: RgbImage,
    siamese: Array4<f32>,
    edge_hog: Vec<f32>,
}

impl IconRecognizer {
    pub fn load(model_dir: &Path, copies: usize, threads: usize) -> Result<Self> {
        let copies = copies.clamp(1, 4);
        let threads = threads.clamp(1, 8);
        let detector = SessionPool::load(&model_dir.join("detect.onnx"), copies, threads)?;
        let fallback_path = model_dir.join("detect_icon3.onnx");
        let fallback_detector = if fallback_path.is_file() {
            Some(SessionPool::load(&fallback_path, 1, threads)?)
        } else {
            None
        };
        let matcher = SessionPool::load(&model_dir.join("simvgg19.onnx"), copies, threads)?;
        Ok(Self {
            detector,
            fallback_detector,
            matcher,
        })
    }

    pub fn warm_up(&self) -> Result<()> {
        let background = RgbImage::from_pixel(300, 200, Rgb([127, 127, 127]));
        let detector_result = self.detect_tiled(&self.detector, &background);
        if let Err(error) = detector_result {
            tracing::warn!(error = %error, "v4 detector warm-up failed");
        }
        let first = Array4::<f32>::zeros((1, 3, 64, 64));
        self.siamese(&first, &first)?;
        Ok(())
    }

    pub fn primary_plan(
        &self,
        background: &RgbImage,
        questions: &[RgbaImage],
        max_attempts: usize,
    ) -> Result<MatchPlan> {
        let question_assets = questions
            .iter()
            .map(question_asset)
            .collect::<Result<Vec<_>>>()?;
        let candidates = self.detect_tiled(&self.detector, background)?;
        if candidates.len() < questions.len() {
            return Err(other_without_source(&format!(
                "v4 detector found {} icons, need {}",
                candidates.len(),
                questions.len()
            )));
        }
        let ranked = self.rank(background, &question_assets, &candidates, max_attempts)?;
        Ok(MatchPlan {
            clicks: ranked,
            candidate_count: candidates.len(),
            phase: "primary",
        })
    }

    pub fn fallback_plan(
        &self,
        background: &RgbImage,
        questions: &[RgbaImage],
        max_attempts: usize,
    ) -> Result<MatchPlan> {
        let question_assets = questions
            .iter()
            .map(question_asset)
            .collect::<Result<Vec<_>>>()?;
        let mut candidates = self.detect_tiled(&self.detector, background)?;
        if let Some(ref fallback) = self.fallback_detector {
            if let Ok(mut extra) = self.detect_tiled(fallback, background) {
                candidates.append(&mut extra);
            }
        }
        candidates.extend(generic_candidates(background));
        let candidates = dedupe_candidates(candidates, MAX_CANDIDATES);
        if candidates.len() < questions.len() {
            return Err(other_without_source(&format!(
                "v4 fallback detector found {} icons, need {}",
                candidates.len(),
                questions.len()
            )));
        }
        let ranked = self.rank(background, &question_assets, &candidates, max_attempts)?;
        Ok(MatchPlan {
            clicks: ranked,
            candidate_count: candidates.len(),
            phase: "fallback",
        })
    }

    fn rank(
        &self,
        background: &RgbImage,
        questions: &[QuestionAsset],
        candidates: &[Candidate],
        limit: usize,
    ) -> Result<Vec<Vec<(f64, f64)>>> {
        let candidate_assets = candidates
            .iter()
            .map(|candidate| candidate_asset(background, *candidate))
            .collect::<Vec<_>>();
        let mut scores = vec![vec![0f32; candidates.len()]; questions.len()];
        for (q_index, question) in questions.iter().enumerate() {
            for (c_index, candidate) in candidate_assets.iter().enumerate() {
                let sim = self.siamese(&question.siamese, &candidate.siamese)?;
                let edge_hog = question
                    .edge_hogs
                    .iter()
                    .map(|feature| cosine(feature, &candidate.edge_hog))
                    .fold(f32::NEG_INFINITY, f32::max);
                let local = local_contrast_score(&candidate.crop, &question.rotated_masks);
                scores[q_index][c_index] = SIM_WEIGHT * normalize(sim, SIM_RANGE)
                    + EDGE_HOG_WEIGHT * normalize(edge_hog, EDGE_HOG_RANGE)
                    + LOCAL_WEIGHT * normalize(local, LOCAL_RANGE);
            }
        }

        let width = background.width() as f64;
        let height = background.height() as f64;
        let mut permutations_list = Vec::new();
        permutations(
            candidates.len(),
            questions.len(),
            &mut Vec::new(),
            &mut permutations_list,
        );
        let mut ranking = permutations_list
            .into_iter()
            .map(|permutation| {
                let score = permutation
                    .iter()
                    .enumerate()
                    .map(|(question, candidate)| scores[question][*candidate])
                    .sum::<f32>();
                (score, permutation)
            })
            .collect::<Vec<_>>();
        ranking.sort_by(|left, right| right.0.total_cmp(&left.0));
        ranking.truncate(limit.max(1));
        Ok(ranking
            .into_iter()
            .map(|(_, permutation)| {
                permutation
                    .into_iter()
                    .map(|index| {
                        let (x, y) = candidates[index].center();
                        (x / width, y / height)
                    })
                    .collect()
            })
            .collect())
    }

    fn siamese(&self, first: &Array4<f32>, second: &Array4<f32>) -> Result<f32> {
        let session = self.matcher.next();
        let session = session
            .lock()
            .map_err(|_| other_without_source("v4 matcher mutex poisoned"))?;
        let outputs = session
            .run(
                ort::inputs![
                    "img1" => first.view(),
                    "img2" => second.view()
                ]
                .map_err(|e| other("build v4 Siamese inputs failed", e))?,
            )
            .map_err(|e| other("v4 Siamese inference failed", e))?;
        let output = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| other("extract v4 Siamese output failed", e))?;
        let value = output.iter().next().copied().unwrap_or(0.0);
        Ok(1.0 / (1.0 + (-value.clamp(-50.0, 50.0)).exp()))
    }

    fn detect_tiled(&self, pool: &SessionPool, image: &RgbImage) -> Result<Vec<Candidate>> {
        let mut raw = self.infer(pool, image)?;
        let flipped = image::imageops::flip_horizontal(image);
        for candidate in self.infer(pool, &flipped)? {
            raw.push(Candidate {
                x1: image.width().saturating_sub(candidate.x2),
                y1: candidate.y1,
                x2: image.width().saturating_sub(candidate.x1),
                y2: candidate.y2,
                confidence: candidate.confidence,
            });
        }
        let upscaled = resize(
            image,
            image.width() * 2,
            image.height() * 2,
            FilterType::CatmullRom,
        );
        for candidate in self.infer(pool, &upscaled)? {
            raw.push(Candidate {
                x1: candidate.x1 / 2,
                y1: candidate.y1 / 2,
                x2: candidate.x2.div_ceil(2),
                y2: candidate.y2.div_ceil(2),
                confidence: candidate.confidence,
            });
        }

        let tile_w = image.width().min(200);
        let tile_h = image.height().min(140);
        let x_positions = unique_positions([
            0,
            image.width().saturating_sub(tile_w),
            image.width().saturating_sub(tile_w) / 2,
        ]);
        let y_positions = unique_positions([
            0,
            image.height().saturating_sub(tile_h),
            image.height().saturating_sub(tile_h) / 2,
        ]);
        for y0 in y_positions {
            for &x0 in &x_positions {
                let tile = image::imageops::crop_imm(image, x0, y0, tile_w, tile_h).to_image();
                for candidate in self.infer(pool, &tile)? {
                    raw.push(Candidate {
                        x1: candidate.x1 + x0,
                        y1: candidate.y1 + y0,
                        x2: candidate.x2 + x0,
                        y2: candidate.y2 + y0,
                        confidence: candidate.confidence,
                    });
                }
            }
        }

        let mut filtered = raw
            .iter()
            .copied()
            .filter(|box_| {
                let ratio = box_.width() as f32 / box_.height().max(1) as f32;
                (28..=85).contains(&box_.width())
                    && (28..=85).contains(&box_.height())
                    && (0.45..=1.8).contains(&ratio)
            })
            .collect::<Vec<_>>();
        if filtered.len() < 3 {
            filtered = raw
                .into_iter()
                .filter(|box_| {
                    let ratio = box_.width() as f32 / box_.height().max(1) as f32;
                    (20..=105).contains(&box_.width())
                        && (20..=105).contains(&box_.height())
                        && (0.35..=2.4).contains(&ratio)
                })
                .collect();
        }
        filtered.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
        let mut unique = Vec::new();
        for candidate in filtered {
            if unique.iter().all(|old| iou(candidate, *old) < 0.35) {
                unique.push(candidate);
            }
            if unique.len() >= MAX_CANDIDATES {
                break;
            }
        }
        Ok(unique)
    }

    fn infer(&self, pool: &SessionPool, image: &RgbImage) -> Result<Vec<Candidate>> {
        let width = image.width();
        let height = image.height();
        let ratio = (DETECT_SIZE as f32 / width as f32).min(DETECT_SIZE as f32 / height as f32);
        let new_width = ((width as f32 * ratio).round() as u32).max(1);
        let new_height = ((height as f32 * ratio).round() as u32).max(1);
        let pad_x = (DETECT_SIZE - new_width) / 2;
        let pad_y = (DETECT_SIZE - new_height) / 2;
        let resized = resize(image, new_width, new_height, FilterType::Triangle);
        let mut tensor = Array4::<f32>::from_elem(
            (1, 3, DETECT_SIZE as usize, DETECT_SIZE as usize),
            114.0 / 255.0,
        );
        for y in 0..new_height {
            for x in 0..new_width {
                let pixel = resized.get_pixel(x, y);
                let target_x = (x + pad_x) as usize;
                let target_y = (y + pad_y) as usize;
                tensor[[0, 0, target_y, target_x]] = pixel[0] as f32 / 255.0;
                tensor[[0, 1, target_y, target_x]] = pixel[1] as f32 / 255.0;
                tensor[[0, 2, target_y, target_x]] = pixel[2] as f32 / 255.0;
            }
        }

        let session = pool.next();
        let session = session
            .lock()
            .map_err(|_| other_without_source("v4 detector mutex poisoned"))?;
        let outputs = session
            .run(
                ort::inputs!["images" => tensor.view()]
                    .map_err(|e| other("build v4 YOLO inputs failed", e))?,
            )
            .map_err(|e| other("v4 YOLO inference failed", e))?;
        let output = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| other("extract v4 YOLO output failed", e))?;
        let shape = output.shape();
        if shape.len() != 3 || shape[1] < 5 {
            return Err(other_without_source(&format!(
                "unexpected v4 YOLO output shape: {shape:?}"
            )));
        }
        let count = shape[2];
        let channels = shape[1];
        let mut boxes = Vec::new();
        let mut scores = Vec::new();
        for index in 0..count {
            let mut score = 0.0f32;
            for channel in 4..channels {
                score = score.max(output[[0, channel, index]]);
            }
            if score < DETECT_CONF {
                continue;
            }
            let cx = output[[0, 0, index]];
            let cy = output[[0, 1, index]];
            let bw = output[[0, 2, index]];
            let bh = output[[0, 3, index]];
            let x1 = ((cx - bw / 2.0 - pad_x as f32) / ratio).round() as i32;
            let y1 = ((cy - bh / 2.0 - pad_y as f32) / ratio).round() as i32;
            let x2 = ((cx + bw / 2.0 - pad_x as f32) / ratio).round() as i32;
            let y2 = ((cy + bh / 2.0 - pad_y as f32) / ratio).round() as i32;
            let x1 = x1.clamp(0, width.saturating_sub(1) as i32) as u32;
            let y1 = y1.clamp(0, height.saturating_sub(1) as i32) as u32;
            let x2 = x2.clamp(x1 as i32 + 1, width as i32) as u32;
            let y2 = y2.clamp(y1 as i32 + 1, height as i32) as u32;
            boxes.push([x1, y1, x2 - x1, y2 - y1]);
            scores.push(score);
        }
        Ok(nms(&boxes, &scores, DETECT_CONF, DETECT_IOU))
    }
}

fn unique_positions(mut values: [u32; 3]) -> Vec<u32> {
    values.sort_unstable();
    let mut output = values.to_vec();
    output.dedup();
    output
}

fn question_asset(png: &RgbaImage) -> Result<QuestionAsset> {
    let mut min_x = png.width();
    let mut min_y = png.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let mut found = false;
    for (x, y, pixel) in png.enumerate_pixels() {
        if pixel[3] > 12 {
            found = true;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    if !found {
        return Err(other_without_source(
            "question image has an empty alpha channel",
        ));
    }
    let alpha = image::imageops::crop_imm(png, min_x, min_y, max_x - min_x + 1, max_y - min_y + 1)
        .to_image();
    let alpha = convert_alpha(&alpha);
    let side = alpha.width().max(alpha.height());
    let mut normalized = GrayImage::new(side, side);
    let x0 = (side - alpha.width()) / 2;
    let y0 = (side - alpha.height()) / 2;
    image::imageops::overlay(&mut normalized, &alpha, x0 as i64, y0 as i64);
    let rendered = GrayImage::from_fn(side, side, |x, y| {
        Luma([255u8.saturating_sub(normalized.get_pixel(x, y)[0])])
    });
    let siamese = image_to_tensor(&DynamicImage::ImageLuma8(rendered.clone()).to_rgb8());
    let edge_hogs = (-180..180)
        .step_by(15)
        .map(|angle| {
            let rotated = rotate_gray(&rendered, angle as f32, 255);
            hog_feature(&rotated)
        })
        .collect::<Result<Vec<_>>>()?;
    let rotated_masks = (-180..180)
        .step_by(20)
        .map(|angle| rotate_gray(&alpha, angle as f32, 0))
        .collect();
    Ok(QuestionAsset {
        siamese,
        edge_hogs,
        rotated_masks,
    })
}

fn convert_alpha(image: &image::RgbaImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        Luma([image.get_pixel(x, y)[3]])
    })
}

fn candidate_asset(background: &RgbImage, candidate: Candidate) -> CandidateAsset {
    let pad = 2;
    let x1 = candidate.x1.saturating_sub(pad);
    let y1 = candidate.y1.saturating_sub(pad);
    let x2 = (candidate.x2 + pad).min(background.width());
    let y2 = (candidate.y2 + pad).min(background.height());
    let crop = image::imageops::crop_imm(background, x1, y1, x2 - x1, y2 - y1).to_image();
    CandidateAsset {
        siamese: image_to_tensor(&crop),
        edge_hog: {
            let gray = DynamicImage::ImageRgb8(crop.clone()).to_luma8();
            hog_feature(&gray).unwrap_or_default()
        },
        crop,
    }
}

fn image_to_tensor(image: &RgbImage) -> Array4<f32> {
    let resized = resize(image, SIAMESE_SIZE, SIAMESE_SIZE, FilterType::Triangle);
    let mut tensor = Array4::<f32>::zeros((1, 3, SIAMESE_SIZE as usize, SIAMESE_SIZE as usize));
    for y in 0..SIAMESE_SIZE {
        for x in 0..SIAMESE_SIZE {
            let pixel = resized.get_pixel(x, y);
            tensor[[0, 0, y as usize, x as usize]] = pixel[0] as f32 / 255.0;
            tensor[[0, 1, y as usize, x as usize]] = pixel[1] as f32 / 255.0;
            tensor[[0, 2, y as usize, x as usize]] = pixel[2] as f32 / 255.0;
        }
    }
    tensor
}

fn normalize(value: f32, bounds: (f32, f32)) -> f32 {
    ((value - bounds.0) / (bounds.1 - bounds.0).max(1e-9)).clamp(0.0, 1.0)
}

fn cosine(first: &[f32], second: &[f32]) -> f32 {
    if first.len() != second.len() || first.is_empty() {
        return 0.0;
    }
    first.iter().zip(second).map(|(a, b)| a * b).sum::<f32>()
}

fn rotate_gray(image: &GrayImage, angle: f32, border: u8) -> GrayImage {
    let width = image.width() as f32;
    let height = image.height() as f32;
    let radians = angle.to_radians();
    let (sin, cos) = radians.sin_cos();
    let cx = (width - 1.0) / 2.0;
    let cy = (height - 1.0) / 2.0;
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let source_x = cos * dx + sin * dy + cx;
        let source_y = -sin * dx + cos * dy + cy;
        if source_x < 0.0 || source_y < 0.0 || source_x > width - 1.0 || source_y > height - 1.0 {
            return Luma([border]);
        }
        let value = bilinear_gray(image, source_x, source_y);
        Luma([value])
    })
}

fn bilinear_gray(image: &GrayImage, x: f32, y: f32) -> u8 {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(image.width() - 1);
    let y1 = (y0 + 1).min(image.height() - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p00 = image.get_pixel(x0, y0)[0] as f32;
    let p10 = image.get_pixel(x1, y0)[0] as f32;
    let p01 = image.get_pixel(x0, y1)[0] as f32;
    let p11 = image.get_pixel(x1, y1)[0] as f32;
    ((p00 * (1.0 - fx) + p10 * fx) * (1.0 - fy) + (p01 * (1.0 - fx) + p11 * fx) * fy)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn gaussian_blur(image: &GrayImage, sigma: f32) -> GrayImage {
    if sigma <= 0.0 {
        return image.clone();
    }
    let radius = (sigma * 3.0).ceil() as i32;
    let mut kernel = Vec::with_capacity((radius * 2 + 1) as usize);
    let mut sum = 0.0f32;
    for offset in -radius..=radius {
        let value = (-(offset * offset) as f32 / (2.0 * sigma * sigma)).exp();
        kernel.push(value);
        sum += value;
    }
    for value in &mut kernel {
        *value /= sum;
    }
    let width = image.width() as i32;
    let height = image.height() as i32;
    let mut horizontal = GrayImage::new(image.width(), image.height());
    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0;
            for (index, weight) in kernel.iter().enumerate() {
                let source_x = (x + index as i32 - radius).clamp(0, width - 1);
                value += image.get_pixel(source_x as u32, y as u32)[0] as f32 * weight;
            }
            horizontal.put_pixel(
                x as u32,
                y as u32,
                Luma([value.round().clamp(0.0, 255.0) as u8]),
            );
        }
    }
    let mut output = GrayImage::new(image.width(), image.height());
    for y in 0..height {
        for x in 0..width {
            let mut value = 0.0;
            for (index, weight) in kernel.iter().enumerate() {
                let source_y = (y + index as i32 - radius).clamp(0, height - 1);
                value += horizontal.get_pixel(x as u32, source_y as u32)[0] as f32 * weight;
            }
            output.put_pixel(
                x as u32,
                y as u32,
                Luma([value.round().clamp(0.0, 255.0) as u8]),
            );
        }
    }
    output
}

fn hog_feature(image: &GrayImage) -> Result<Vec<f32>> {
    let gray = resize(image, HOG_SIZE, HOG_SIZE, FilterType::Triangle);
    let edges = canny_like(&gray);
    let cell = 8usize;
    let cells = (HOG_SIZE / cell as u32) as usize;
    let mut histograms = vec![[0f32; 9]; cells * cells];
    for y in 0..HOG_SIZE as usize {
        for x in 0..HOG_SIZE as usize {
            let magnitude = edges[y * HOG_SIZE as usize + x];
            if magnitude < 8.0 {
                continue;
            }
            let gx = edge_gradient(&gray, x as i32, y as i32, true);
            let gy = edge_gradient(&gray, x as i32, y as i32, false);
            let mut angle = gy.atan2(gx).to_degrees();
            if angle < 0.0 {
                angle += 180.0;
            }
            if angle >= 180.0 {
                angle -= 180.0;
            }
            let bin = ((angle / 20.0).round() as usize).min(8);
            histograms[(y / cell) * cells + x / cell][bin] += magnitude;
        }
    }
    let mut feature = Vec::with_capacity((cells - 1) * (cells - 1) * 36);
    for cell_y in 0..cells - 1 {
        for cell_x in 0..cells - 1 {
            let mut block = Vec::with_capacity(36);
            for y in cell_y..cell_y + 2 {
                for x in cell_x..cell_x + 2 {
                    block.extend_from_slice(&histograms[y * cells + x]);
                }
            }
            l2_hys(&mut block);
            feature.extend(block);
        }
    }
    let norm = feature.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for value in &mut feature {
            *value /= norm;
        }
    }
    Ok(feature)
}

fn l2_hys(values: &mut [f32]) {
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm <= 1e-9 {
        return;
    }
    for value in values.iter_mut() {
        *value = (*value / norm).min(0.2);
    }
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 1e-9 {
        for value in values.iter_mut() {
            *value /= norm;
        }
    }
}

fn edge_gradient(image: &GrayImage, x: i32, y: i32, horizontal: bool) -> f32 {
    let width = image.width() as i32;
    let height = image.height() as i32;
    if horizontal {
        let left = image.get_pixel((x - 1).clamp(0, width - 1) as u32, y as u32)[0] as f32;
        let right = image.get_pixel((x + 1).clamp(0, width - 1) as u32, y as u32)[0] as f32;
        right - left
    } else {
        let top = image.get_pixel(x as u32, (y - 1).clamp(0, height - 1) as u32)[0] as f32;
        let bottom = image.get_pixel(x as u32, (y + 1).clamp(0, height - 1) as u32)[0] as f32;
        bottom - top
    }
}

fn canny_like(image: &GrayImage) -> Vec<f32> {
    let mut edges = vec![0f32; (image.width() * image.height()) as usize];
    for y in 0..image.height() as i32 {
        for x in 0..image.width() as i32 {
            let gx = edge_gradient(image, x, y, true);
            let gy = edge_gradient(image, x, y, false);
            let magnitude = (gx * gx + gy * gy).sqrt();
            if magnitude >= 24.0 {
                edges[y as usize * image.width() as usize + x as usize] = magnitude;
            }
        }
    }
    edges
}

fn local_contrast_score(patch: &RgbImage, masks: &[GrayImage]) -> f32 {
    let gray = DynamicImage::ImageRgb8(patch.clone()).to_luma8();
    let blurred = gaussian_blur(&gray, 4.0);
    let source = GrayImage::from_fn(gray.width(), gray.height(), |x, y| {
        let value =
            (gray.get_pixel(x, y)[0] as i16 - blurred.get_pixel(x, y)[0] as i16).unsigned_abs();
        Luma([(value.saturating_mul(2)).min(255) as u8])
    });
    let largest = source.width().min(source.height()).max(1);
    let smallest = (largest as f32 * 0.55).max(20.0) as u32;
    let mut best = 0.0f32;
    for mask in masks {
        let mask_largest = mask.width().max(mask.height()).max(1);
        for target in (smallest..=largest).step_by(3) {
            let scale = target as f32 / mask_largest as f32;
            let width = ((mask.width() as f32 * scale).round() as u32).max(4);
            let height = ((mask.height() as f32 * scale).round() as u32).max(4);
            if width > source.width() || height > source.height() {
                continue;
            }
            if width < 4 || height < 4 {
                continue;
            }
            let resized = resize(mask, width, height, FilterType::Nearest);
            if resized.pixels().filter(|pixel| pixel[0] > 40).count() < 20 {
                continue;
            }
            let max_x = source.width().saturating_sub(width);
            let max_y = source.height().saturating_sub(height);
            for y0 in candidate_offsets(max_y) {
                for x0 in candidate_offsets(max_x) {
                    let mut numerator = 0.0f32;
                    let mut source_energy = 0.0f32;
                    let mut template_energy = 0.0f32;
                    for y in (0..height).step_by(2) {
                        for x in (0..width).step_by(2) {
                            let template = resized.get_pixel(x, y)[0];
                            if template <= 40 {
                                continue;
                            }
                            let value = source.get_pixel(x0 + x, y0 + y)[0] as f32;
                            let weight = template as f32 / 255.0;
                            numerator += value * weight;
                            source_energy += value * value;
                            template_energy += weight * weight;
                        }
                    }
                    let score =
                        numerator / (source_energy.sqrt() * template_energy.sqrt()).max(1e-9);
                    best = best.max(score);
                }
            }
        }
    }
    best.clamp(0.0, 1.0)
}

fn candidate_offsets(maximum: u32) -> Vec<u32> {
    let center = maximum / 2;
    unique_positions([center.saturating_sub(3), center, (center + 3).min(maximum)])
}

fn permutations(
    candidate_count: usize,
    question_count: usize,
    current: &mut Vec<usize>,
    output: &mut Vec<Vec<usize>>,
) {
    if current.len() == question_count {
        output.push(current.clone());
        return;
    }
    for index in 0..candidate_count {
        if current.contains(&index) {
            continue;
        }
        current.push(index);
        permutations(candidate_count, question_count, current, output);
        current.pop();
    }
}

fn nms(
    boxes: &[[u32; 4]],
    scores: &[f32],
    score_threshold: f32,
    iou_threshold: f32,
) -> Vec<Candidate> {
    let mut order = (0..boxes.len()).collect::<Vec<_>>();
    order.sort_by(|left, right| scores[*right].total_cmp(&scores[*left]));
    let mut selected = Vec::new();
    while let Some(index) = order.first().copied() {
        if scores[index] < score_threshold {
            break;
        }
        let current = Candidate {
            x1: boxes[index][0],
            y1: boxes[index][1],
            x2: boxes[index][0] + boxes[index][2],
            y2: boxes[index][1] + boxes[index][3],
            confidence: scores[index],
        };
        selected.push(current);
        order.retain(|other| {
            let candidate = Candidate {
                x1: boxes[*other][0],
                y1: boxes[*other][1],
                x2: boxes[*other][0] + boxes[*other][2],
                y2: boxes[*other][1] + boxes[*other][3],
                confidence: scores[*other],
            };
            iou(current, candidate) <= iou_threshold
        });
    }
    selected
}

fn iou(first: Candidate, second: Candidate) -> f32 {
    let x1 = first.x1.max(second.x1);
    let y1 = first.y1.max(second.y1);
    let x2 = first.x2.min(second.x2);
    let y2 = first.y2.min(second.y2);
    let intersection = x2.saturating_sub(x1) as f32 * y2.saturating_sub(y1) as f32;
    let union = first.width() as f32 * first.height() as f32
        + second.width() as f32 * second.height() as f32
        - intersection;
    intersection / union.max(1.0)
}

fn dedupe_candidates(mut candidates: Vec<Candidate>, limit: usize) -> Vec<Candidate> {
    candidates.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    let mut unique = Vec::new();
    for candidate in candidates {
        if candidate.width() < 20 || candidate.height() < 20 {
            continue;
        }
        if unique.iter().all(|old| iou(candidate, *old) < 0.32) {
            unique.push(candidate);
        }
        if unique.len() >= limit {
            break;
        }
    }
    unique
}

fn generic_candidates(background: &RgbImage) -> Vec<Candidate> {
    let gray = DynamicImage::ImageRgb8(background.clone()).to_luma8();
    let width = gray.width();
    let height = gray.height();
    let mut proposals = Vec::new();
    for sigma in [1.5f32, 2.0, 2.5, 3.0, 3.5, 4.0] {
        let blurred = gaussian_blur(&gray, sigma);
        let edges = canny_like(&blurred);
        let mask = edges
            .iter()
            .map(|value| if *value >= 20.0 { 255u8 } else { 0 })
            .collect::<Vec<_>>();
        proposals.extend(components(&mask, width, height, 80));
    }
    let blurred = gaussian_blur(&gray, 6.0);
    let local = GrayImage::from_fn(width, height, |x, y| {
        let value = (gray.get_pixel(x, y)[0] as i16 - blurred.get_pixel(x, y)[0] as i16)
            .unsigned_abs()
            .saturating_mul(2)
            .min(255);
        Luma([value as u8])
    });
    for threshold in [50u8, 70, 90, 110] {
        let mask = local
            .pixels()
            .map(|pixel| if pixel[0] > threshold { 255u8 } else { 0 })
            .collect::<Vec<_>>();
        proposals.extend(components(&mask, width, height, 100));
    }
    proposals.sort_by(|left, right| {
        candidate_priority(background, *right).total_cmp(&candidate_priority(background, *left))
    });
    proposals.truncate(12);
    proposals
}

fn components(mask: &[u8], width: u32, height: u32, min_area: u32) -> Vec<Candidate> {
    let mut visited = vec![false; mask.len()];
    let mut output = Vec::new();
    for start in 0..mask.len() {
        if visited[start] || mask[start] == 0 {
            continue;
        }
        let mut queue = VecDeque::new();
        queue.push_back(start);
        visited[start] = true;
        let mut min_x = width;
        let mut min_y = height;
        let mut max_x = 0;
        let mut max_y = 0;
        let mut area = 0u32;
        while let Some(index) = queue.pop_front() {
            let x = index as u32 % width;
            let y = index as u32 / width;
            area += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let next = ny as usize * width as usize + nx as usize;
                    if !visited[next] && mask[next] != 0 {
                        visited[next] = true;
                        queue.push_back(next);
                    }
                }
            }
        }
        let box_width = max_x - min_x + 1;
        let box_height = max_y - min_y + 1;
        let ratio = box_width as f32 / box_height.max(1) as f32;
        if (28..=115).contains(&box_width)
            && (28..=115).contains(&box_height)
            && area >= min_area
            && (0.4..=2.2).contains(&ratio)
        {
            output.push(Candidate {
                x1: min_x,
                y1: min_y,
                x2: max_x + 1,
                y2: max_y + 1,
                confidence: (area as f32 / 1200.0).min(1.0),
            });
        }
    }
    output
}

fn candidate_priority(background: &RgbImage, candidate: Candidate) -> f32 {
    let patch = image::imageops::crop_imm(
        background,
        candidate.x1,
        candidate.y1,
        candidate.width(),
        candidate.height(),
    )
    .to_image();
    let gray = DynamicImage::ImageRgb8(patch).to_luma8();
    let edges = canny_like(&gray);
    let density = edges.iter().sum::<f32>() / (255.0 * edges.len().max(1) as f32);
    let squareness = candidate.width().min(candidate.height()) as f32
        / candidate.width().max(candidate.height()).max(1) as f32;
    candidate.width() as f32 * candidate.height() as f32 * (0.4 + density) * (0.5 + squareness)
}

pub fn decode_background(bytes: &[u8]) -> Result<RgbImage> {
    Ok(image::load_from_memory(bytes)
        .map_err(parse_error)?
        .to_rgb8())
}

pub fn decode_question(bytes: &[u8]) -> Result<RgbaImage> {
    Ok(image::load_from_memory(bytes)
        .map_err(parse_error)?
        .to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hog_features_are_finite_and_normalized() {
        let image = GrayImage::from_fn(64, 64, |x, y| {
            Luma([if (x / 8 + y / 8) % 2 == 0 { 220 } else { 20 }])
        });
        let feature = hog_feature(&image).unwrap();
        assert_eq!(feature.len(), 1764);
        assert!(feature.iter().all(|value| value.is_finite()));
        let norm = feature
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn permutation_ranking_handles_three_questions() {
        let mut output = Vec::new();
        permutations(4, 3, &mut Vec::new(), &mut output);
        assert_eq!(output.len(), 24);
    }
}
