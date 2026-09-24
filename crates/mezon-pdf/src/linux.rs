use std::ffi::{CStr, c_char, c_double, c_int, c_uchar, c_void};
use std::sync::OnceLock;

use libloading::Library;

const CAIRO_FORMAT_ARGB32: c_int = 0;
const CAIRO_STATUS_SUCCESS: c_int = 0;

const POPPLER_SONAMES: [&str; 2] = ["libpoppler-glib.so.8", "libpoppler-glib.so"];
const CAIRO_SONAMES: [&str; 2] = ["libcairo.so.2", "libcairo.so"];

type PopplerDocument = c_void;
type PopplerPage = c_void;
type CairoSurface = c_void;
type CairoContext = c_void;

type DocumentNewFromData = unsafe extern "C" fn(
    *mut c_char,
    c_int,
    *const c_char,
    *mut *mut glib::ffi::GError,
) -> *mut PopplerDocument;
type DocumentGetNPages = unsafe extern "C" fn(*mut PopplerDocument) -> c_int;
type DocumentGetPage = unsafe extern "C" fn(*mut PopplerDocument, c_int) -> *mut PopplerPage;
type PageGetSize = unsafe extern "C" fn(*mut PopplerPage, *mut c_double, *mut c_double);
type PageRender = unsafe extern "C" fn(*mut PopplerPage, *mut CairoContext);
type ImageSurfaceCreate = unsafe extern "C" fn(c_int, c_int, c_int) -> *mut CairoSurface;
type SurfaceStatus = unsafe extern "C" fn(*mut CairoSurface) -> c_int;
type SurfaceFlush = unsafe extern "C" fn(*mut CairoSurface);
type SurfaceDestroy = unsafe extern "C" fn(*mut CairoSurface);
type ImageSurfaceGetData = unsafe extern "C" fn(*mut CairoSurface) -> *mut c_uchar;
type ImageSurfaceGetStride = unsafe extern "C" fn(*mut CairoSurface) -> c_int;
type ContextCreate = unsafe extern "C" fn(*mut CairoSurface) -> *mut CairoContext;
type ContextStatus = unsafe extern "C" fn(*mut CairoContext) -> c_int;
type ContextDestroy = unsafe extern "C" fn(*mut CairoContext);
type SetSourceRgb = unsafe extern "C" fn(*mut CairoContext, c_double, c_double, c_double);
type Paint = unsafe extern "C" fn(*mut CairoContext);
type Scale = unsafe extern "C" fn(*mut CairoContext, c_double, c_double);

macro_rules! symbol {
    ($library:expr, $name:literal, $signature:ty) => {
        *unsafe { $library.get::<$signature>(concat!($name, "\0").as_bytes()) }
            .map_err(|error| format!("{}: {error}", $name))?
    };
}

struct Backend {
    document_new_from_data: DocumentNewFromData,
    document_get_n_pages: DocumentGetNPages,
    document_get_page: DocumentGetPage,
    page_get_size: PageGetSize,
    page_render: PageRender,
    image_surface_create: ImageSurfaceCreate,
    surface_status: SurfaceStatus,
    surface_flush: SurfaceFlush,
    surface_destroy: SurfaceDestroy,
    image_surface_get_data: ImageSurfaceGetData,
    image_surface_get_stride: ImageSurfaceGetStride,
    context_create: ContextCreate,
    context_status: ContextStatus,
    context_destroy: ContextDestroy,
    set_source_rgb: SetSourceRgb,
    paint: Paint,
    scale: Scale,
    _poppler: Library,
    _cairo: Library,
}

unsafe impl Send for Backend {}
unsafe impl Sync for Backend {}

fn open_any(sonames: &[&str]) -> Result<Library, String> {
    let mut last = format!("{} not found", sonames[0]);
    for soname in sonames {
        match unsafe { Library::new(soname) } {
            Ok(library) => return Ok(library),
            Err(error) => last = format!("{soname}: {error}"),
        }
    }
    Err(last)
}

impl Backend {
    fn load() -> Result<Self, String> {
        let poppler = open_any(&POPPLER_SONAMES)?;
        let cairo = open_any(&CAIRO_SONAMES)?;
        Ok(Self {
            document_new_from_data: symbol!(
                poppler,
                "poppler_document_new_from_data",
                DocumentNewFromData
            ),
            document_get_n_pages: symbol!(
                poppler,
                "poppler_document_get_n_pages",
                DocumentGetNPages
            ),
            document_get_page: symbol!(poppler, "poppler_document_get_page", DocumentGetPage),
            page_get_size: symbol!(poppler, "poppler_page_get_size", PageGetSize),
            page_render: symbol!(poppler, "poppler_page_render", PageRender),
            image_surface_create: symbol!(cairo, "cairo_image_surface_create", ImageSurfaceCreate),
            surface_status: symbol!(cairo, "cairo_surface_status", SurfaceStatus),
            surface_flush: symbol!(cairo, "cairo_surface_flush", SurfaceFlush),
            surface_destroy: symbol!(cairo, "cairo_surface_destroy", SurfaceDestroy),
            image_surface_get_data: symbol!(
                cairo,
                "cairo_image_surface_get_data",
                ImageSurfaceGetData
            ),
            image_surface_get_stride: symbol!(
                cairo,
                "cairo_image_surface_get_stride",
                ImageSurfaceGetStride
            ),
            context_create: symbol!(cairo, "cairo_create", ContextCreate),
            context_status: symbol!(cairo, "cairo_status", ContextStatus),
            context_destroy: symbol!(cairo, "cairo_destroy", ContextDestroy),
            set_source_rgb: symbol!(cairo, "cairo_set_source_rgb", SetSourceRgb),
            paint: symbol!(cairo, "cairo_paint", Paint),
            scale: symbol!(cairo, "cairo_scale", Scale),
            _poppler: poppler,
            _cairo: cairo,
        })
    }
}

fn backend() -> Result<&'static Backend, &'static str> {
    static BACKEND: OnceLock<Result<Backend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(Backend::load)
        .as_ref()
        .map_err(String::as_str)
}

pub fn is_available() -> bool {
    backend().is_ok()
}

pub fn unavailable_reason() -> Option<String> {
    backend()
        .err()
        .map(|error| format!("pdf rendering needs the poppler and cairo libraries ({error})"))
}

struct Page {
    page: *mut PopplerPage,
}

impl Drop for Page {
    fn drop(&mut self) {
        unsafe { glib::gobject_ffi::g_object_unref(self.page.cast()) };
    }
}

struct Surface {
    surface: *mut CairoSurface,
    destroy: SurfaceDestroy,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.surface) };
    }
}

struct Context {
    context: *mut CairoContext,
    destroy: ContextDestroy,
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe { (self.destroy)(self.context) };
    }
}

pub struct Document {
    document: *mut PopplerDocument,
    pages: usize,
    _bytes: Vec<u8>,
}

unsafe impl Send for Document {}

impl Drop for Document {
    fn drop(&mut self) {
        unsafe { glib::gobject_ffi::g_object_unref(self.document.cast()) };
    }
}

impl Document {
    pub fn open(mut bytes: Vec<u8>) -> anyhow::Result<Self> {
        let backend = backend().map_err(|error| anyhow::anyhow!("{error}"))?;
        if bytes.is_empty() {
            anyhow::bail!("file is not a readable pdf");
        }
        let length = c_int::try_from(bytes.len())?;
        let mut error = std::ptr::null_mut();
        let document = unsafe {
            (backend.document_new_from_data)(
                bytes.as_mut_ptr().cast::<c_char>(),
                length,
                std::ptr::null(),
                &mut error,
            )
        };
        if document.is_null() {
            anyhow::bail!("failed to open pdf: {}", take_glib_error(error));
        }
        let pages = unsafe { (backend.document_get_n_pages)(document) }.max(0) as usize;
        Ok(Self {
            document,
            pages,
            _bytes: bytes,
        })
    }

    pub fn page_count(&self) -> usize {
        self.pages
    }

    pub fn page_size(&self, index: usize) -> anyhow::Result<(f32, f32)> {
        let backend = backend().map_err(|error| anyhow::anyhow!("{error}"))?;
        let page = self.page(backend, index)?;
        let (width, height) = size_of_page(backend, &page);
        Ok((width as f32, height as f32))
    }

    pub fn render_page(&self, index: usize, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
        let backend = backend().map_err(|error| anyhow::anyhow!("{error}"))?;
        let page = self.page(backend, index)?;
        let (page_width, page_height) = size_of_page(backend, &page);

        let surface = Surface {
            surface: unsafe {
                (backend.image_surface_create)(CAIRO_FORMAT_ARGB32, width as c_int, height as c_int)
            },
            destroy: backend.surface_destroy,
        };
        if unsafe { (backend.surface_status)(surface.surface) } != CAIRO_STATUS_SUCCESS {
            anyhow::bail!("failed to allocate a {width}x{height} pdf surface");
        }
        {
            let context = Context {
                context: unsafe { (backend.context_create)(surface.surface) },
                destroy: backend.context_destroy,
            };
            if unsafe { (backend.context_status)(context.context) } != CAIRO_STATUS_SUCCESS {
                anyhow::bail!("failed to create a pdf drawing context");
            }
            unsafe {
                (backend.set_source_rgb)(context.context, 1.0, 1.0, 1.0);
                (backend.paint)(context.context);
                (backend.scale)(
                    context.context,
                    width as c_double / page_width.max(1.0),
                    height as c_double / page_height.max(1.0),
                );
                (backend.page_render)(page.page, context.context);
            }
        }
        unsafe { (backend.surface_flush)(surface.surface) };

        let stride = unsafe { (backend.image_surface_get_stride)(surface.surface) }.max(0) as usize;
        let data = unsafe { (backend.image_surface_get_data)(surface.surface) };
        let row_bytes = width as usize * 4;
        if data.is_null() || stride < row_bytes {
            anyhow::bail!("the pdf surface exposed no usable pixels");
        }
        let mut bgra = Vec::with_capacity(row_bytes * height as usize);
        for row in 0..height as usize {
            let start = unsafe { data.add(row * stride) };
            bgra.extend_from_slice(unsafe { std::slice::from_raw_parts(start, row_bytes) });
        }
        Ok(bgra)
    }

    fn page(&self, backend: &Backend, index: usize) -> anyhow::Result<Page> {
        let page = unsafe { (backend.document_get_page)(self.document, index as c_int) };
        if page.is_null() {
            anyhow::bail!("pdf page {index} is missing");
        }
        Ok(Page { page })
    }
}

fn size_of_page(backend: &Backend, page: &Page) -> (c_double, c_double) {
    let mut width = 0.0;
    let mut height = 0.0;
    unsafe { (backend.page_get_size)(page.page, &mut width, &mut height) };
    (width, height)
}

fn take_glib_error(error: *mut glib::ffi::GError) -> String {
    if error.is_null() {
        return "unknown error".to_string();
    }
    let message = unsafe { (*error).message };
    let text = if message.is_null() {
        "unknown error".to_string()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    unsafe { glib::ffi::g_error_free(error) };
    text
}
