use std::error::Error;
use std::fs;
use std::io::BufWriter;

use wasnn::Model;
use wasnn_imageproc::{draw_polygon, Polygon, Rect};
use wasnn_ocr::page_layout::{find_text_lines, line_polygon};
use wasnn_ocr::{detect_words, greyscale_image, recognize_text_lines, ZERO_VALUE};
use wasnn_tensor::{Tensor, TensorLayout, TensorView};

/// Read an image from `path` into a CHW tensor.
fn read_image(path: &str) -> Result<Tensor<f32>, Box<dyn Error>> {
    let input_img = image::open(path)?;
    let input_img = input_img.into_rgb8();

    let (width, height) = input_img.dimensions();

    let in_chans = 3;
    let mut float_img = Tensor::zeros(&[in_chans, height as usize, width as usize]);
    for c in 0..in_chans {
        let mut chan_img = float_img.nd_slice_mut([c]);
        for y in 0..height {
            for x in 0..width {
                chan_img[[y as usize, x as usize]] = input_img.get_pixel(x, y)[c] as f32 / 255.0
            }
        }
    }
    Ok(float_img)
}

/// Write a CHW image to a PNG file in `path`.
fn write_image(path: &str, img: TensorView<f32>) -> Result<(), Box<dyn Error>> {
    if img.ndim() != 3 {
        return Err("Expected CHW input".into());
    }

    let img_width = img.shape()[img.ndim() - 1];
    let img_height = img.shape()[img.ndim() - 2];
    let color_type = match img.shape()[img.ndim() - 3] {
        1 => png::ColorType::Grayscale,
        3 => png::ColorType::Rgb,
        4 => png::ColorType::Rgba,
        _ => return Err("Unsupported channel count".into()),
    };

    let mut hwc_img = img.to_owned();
    hwc_img.permute(&[1, 2, 0]); // CHW => HWC

    let out_img = image_from_tensor(hwc_img);
    let file = fs::File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, img_width as u32, img_height as u32);
    encoder.set_color(color_type);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&out_img)?;

    Ok(())
}

/// Convert an NCHW float tensor with values in the range [0, 1] to Vec<u8>
/// with values scaled to [0, 255].
fn image_from_tensor(tensor: TensorView<f32>) -> Vec<u8> {
    tensor
        .iter()
        .map(|x| (x.clamp(0., 1.) * 255.0) as u8)
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut debug = false;
    let args: Vec<_> = std::env::args()
        .filter(|cmd| match cmd.as_ref() {
            "--debug" => {
                debug = true;
                false
            }
            _ => true,
        })
        .collect();

    if args.len() <= 1 {
        println!(
            "Usage: {} <detection_model> <rec_model> <image>",
            args.get(0).map(|s| s.as_str()).unwrap_or("")
        );
        // Exit with non-zero status, but also don't show an error.
        std::process::exit(1);
    }

    let detection_model_name = args.get(1).ok_or("detection model name not specified")?;
    let recognition_model_name = args.get(2).ok_or("recognition model name not specified")?;
    let image_path = args.get(3).ok_or("image path not specified")?;

    let detection_model_bytes = fs::read(detection_model_name)?;
    let detection_model = Model::load(&detection_model_bytes)?;

    let recognition_model_bytes = fs::read(recognition_model_name)?;
    let recognition_model = Model::load(&recognition_model_bytes)?;

    // Read image into CHW tensor.
    let color_img = read_image(image_path).expect("failed to read input image");

    let normalize_pixel = |pixel| pixel + ZERO_VALUE;

    // Convert color CHW tensor to fixed-size greyscale NCHW input expected by model.
    let grey_img = greyscale_image(color_img.view(), normalize_pixel);

    // Find text components (words) in the input image.
    let word_rects = detect_words(grey_img.view(), &detection_model, debug)?;

    // Perform layout analysis to group words into lines, in reading order.
    let [_, img_height, img_width] = grey_img.dims();
    let page_rect = Rect::from_hw(img_height as i32, img_width as i32);
    let line_rects = find_text_lines(&word_rects, page_rect);

    // Perform recognition on the detected text lines.
    let line_texts = recognize_text_lines(
        grey_img.view(),
        &line_rects,
        page_rect,
        &recognition_model,
        debug,
    )?;
    for line in line_texts {
        println!("{}", line);
    }

    if debug {
        println!(
            "Found {} words, {} lines in image of size {}x{}",
            word_rects.len(),
            line_rects.len(),
            img_width,
            img_height
        );

        let mut annotated_img = color_img;

        for line in line_rects {
            let line_poly = Polygon::new(line_polygon(&line));
            draw_polygon(annotated_img.nd_slice_mut([0]), line_poly.vertices(), 0.9); // Red
            draw_polygon(annotated_img.nd_slice_mut([1]), line_poly.vertices(), 0.); // Green
            draw_polygon(annotated_img.nd_slice_mut([2]), line_poly.vertices(), 0.);
            // Blue
        }

        // Write out the annotated input image.
        write_image("ocr-detection-output.png", annotated_img.view())?;
    }

    Ok(())
}
