use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_get_pdf_num_pages")]
pub struct GetPdfNumPagesParams {
    #[schemars(
        description = "Absolute paths should start with `file:///`. Relative paths are relative to any of the user’s current workspace directories.",
        example = "file:///C:/Users/Admin/Documents/example-project/example-document.pdf",
        example = "./example-document.pdf"
    )]
    pub path: String,
}

#[allow(dead_code)]
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_read_pdf_as_images")]
pub struct ReadPdfAsImagesParams {
    #[schemars(
        description = "Absolute paths should start with `file:///`. Relative paths are relative to any of the user’s current workspace directories.",
        example = "file:///C:/Users/Admin/Documents/example-project/example-document.pdf",
        example = "./example-document.pdf"
    )]
    pub path: String,
    #[serde(default = "const_usize::<1>")]
    #[schemars(range(min = 1))]
    pub from_page: usize,
    #[schemars(range(min = 1))]
    pub to_page: Option<usize>,
    #[serde(default = "const_u16::<1024>")]
    #[schemars(
        description = "Number of pixels on the longer side of each output image",
        example = 1024,
        range(min = 1)
    )]
    pub image_dimension: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_read_pdf_as_text")]
pub struct ReadPdfAsTextParams {
    #[schemars(
        description = "Absolute paths should start with `file:///`. Relative paths are relative to any of the user’s current workspace directories.",
        example = "file:///C:/Users/Admin/Documents/example-project/example-document.pdf",
        example = "./example-document.pdf"
    )]
    pub path: String,
    #[serde(default = "const_usize::<1>")]
    #[schemars(range(min = 1))]
    pub from_page: usize,
    #[schemars(description = "Out-of-range is fine", example = None::<usize>, range(min = 1))]
    pub to_page: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_read_pdf_page_as_image")]
pub struct ReadPdfPageAsImageParams {
    #[schemars(
        description = "Absolute paths should start with `file:///`. Relative paths are relative to any of the user’s current workspace directories.",
        example = "file:///C:/Users/Admin/Documents/example-project/example-document.pdf",
        example = "./example-document.pdf"
    )]
    pub path: String,
    #[serde(default = "const_usize::<1>")]
    #[schemars(range(min = 1))]
    pub page: usize,
    #[serde(default = "const_u16::<1024>")]
    #[schemars(
        description = "Number of pixels on the longer side of each output image",
        example = 1024,
        range(min = 1)
    )]
    pub image_dimension: u16,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[repr(transparent)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_get_pdf_num_pages")]
pub struct GetPdfNumPagesResult {
    #[schemars(example = 42)]
    pub num_pages: usize,
}

const fn const_u16<const N: u16>() -> u16 {
    N
}

const fn const_usize<const N: usize>() -> usize {
    N
}
