fn minimal_pdf() -> Vec<u8> {
    let objects: [&str; 4] = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 100] /Contents 4 0 R >>",
        "<< /Length 27 >>\nstream\n0 0 0 rg 20 20 160 60 re f\nendstream",
    ];
    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref_offset = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        pdf.push_str(&format!("{offset:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        objects.len() + 1
    ));
    pdf.into_bytes()
}

fn backend_ready() -> bool {
    if mezon_pdf::is_supported() {
        return true;
    }
    eprintln!(
        "skipping: {}",
        mezon_pdf::unavailable_reason().unwrap_or_else(|| "pdf backend unavailable".to_string())
    );
    false
}

#[test]
fn renders_a_page_to_an_opaque_bitmap() {
    if !backend_ready() {
        return;
    }
    let doc = mezon_pdf::PdfDocument::from_bytes(minimal_pdf()).expect("open pdf");
    assert_eq!(doc.page_count(), 1);

    let size = doc.page_size(0).expect("page size");
    assert!((size.width - 200.0).abs() < 1.0, "width was {}", size.width);
    assert!(
        (size.height - 100.0).abs() < 1.0,
        "height was {}",
        size.height
    );

    let (width, height) = mezon_pdf::fit_page_pixels(&size, 400.0);
    assert_eq!((width, height), (400, 200));

    let bitmap = doc.render_page(0, width, height).expect("render page");
    assert_eq!(bitmap.width, width);
    assert_eq!(bitmap.height, height);
    assert_eq!(bitmap.bgra.len(), (width * height * 4) as usize);
    assert!(bitmap.bgra.chunks_exact(4).all(|pixel| pixel[3] == 255));

    let corner = &bitmap.bgra[0..4];
    assert!(corner[0] > 240 && corner[1] > 240 && corner[2] > 240);
    let centre = ((height / 2 * width + width / 2) * 4) as usize;
    let centre = &bitmap.bgra[centre..centre + 4];
    assert!(centre[0] < 32 && centre[1] < 32 && centre[2] < 32);

    let ink = ink_bounds(&bitmap);
    assert_eq!(ink, (40, 40, 359, 159), "the page must fill the bitmap");
}

fn ink_bounds(bitmap: &mezon_pdf::PdfBitmap) -> (u32, u32, u32, u32) {
    let (mut left, mut top, mut right, mut bottom) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..bitmap.height {
        for x in 0..bitmap.width {
            let at = ((y * bitmap.width + x) * 4) as usize;
            if bitmap.bgra[at] < 200 {
                left = left.min(x);
                top = top.min(y);
                right = right.max(x);
                bottom = bottom.max(y);
            }
        }
    }
    (left, top, right, bottom)
}

#[test]
fn rejects_a_page_index_past_the_end() {
    if !backend_ready() {
        return;
    }
    let doc = mezon_pdf::PdfDocument::from_bytes(minimal_pdf()).expect("open pdf");
    assert!(doc.page_size(1).is_err());
    assert!(doc.render_page(1, 100, 100).is_err());
}

#[test]
fn rejects_bytes_that_are_not_a_pdf() {
    if !backend_ready() {
        return;
    }
    assert!(mezon_pdf::PdfDocument::from_bytes(b"not a pdf at all".to_vec()).is_err());
}
