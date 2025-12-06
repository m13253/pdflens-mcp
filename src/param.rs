use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_get_pdf_num_pages")]
pub struct GetPdfNumPagesParams {
    #[schemars(
        description = "Relative paths are relative to the root of any opened workspaces.",
        example = "file:///home/user/Documents/workspace/document.pdf",
        example = "./document.pdf"
    )]
    pub path: String,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_read_pdf_as_text")]
pub struct ReadPdfAsTextParams {
    #[schemars(
        description = "Relative paths are relative to the root of any opened workspaces.",
        example = "file:///home/user/Documents/workspace/document.pdf",
        example = "./document.pdf"
    )]
    pub path: String,
    #[serde(default = "const_usize::<1>")]
    #[schemars(example = 1, range(min = 1))]
    pub from_page: usize,
    #[schemars(description = "null = last page", example = None::<usize>, range(min = 1))]
    pub to_page: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_convert_pdf_to_images")]
pub struct ReadPdfAsImagesParams {
    #[schemars(
        description = "Relative paths are relative to the root of any opened workspaces.",
        example = "file:///home/user/Documents/workspace/document.pdf",
        example = "./document.pdf"
    )]
    pub path: String,
    #[serde(default = "const_usize::<1>")]
    #[schemars(example = 42, range(min = 1))]
    pub from_page: usize,
    #[schemars(example = 42, range(min = 1))]
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
#[repr(transparent)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_get_pdf_num_pages")]
pub struct GetPdfNumPagesResult {
    #[schemars(example = 42)]
    pub num_pages: usize,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[repr(transparent)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[schemars(title = "pdflens_list_workspace_dirs")]
pub struct ListWorkspaceDirsResults {
    #[schemars(
        example = [
            "file:///home/user/Documents/project",
            "file:///home/user/Documents/another-project"
        ]
    )]
    pub dirs: Vec<String>,
}

const fn const_u16<const N: u16>() -> u16 {
    N
}

const fn const_usize<const N: usize>() -> usize {
    N
}
