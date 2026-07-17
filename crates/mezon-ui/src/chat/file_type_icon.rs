use crate::components::primitives::IconName;

pub fn file_type_icon(filetype: &str) -> IconName {
    file_type_icon_for(filetype, "")
}

pub fn file_type_icon_for(filetype: &str, filename: &str) -> IconName {
    let key = filetype.trim().to_ascii_lowercase();
    let from_mime = match key.as_str() {
        "text/plain" | "txt" => Some(IconName::FileThumbDoc),
        "text/html" | "text/css" | "application/xml" | "image/svg+xml" | "html" | "css"
        | "xml" => Some(IconName::FileThumbCode),
        "application/javascript" | "application/json" | "json" | "js" => {
            Some(IconName::FileThumbJson)
        }
        "audio/mpeg" | "audio/mp3" | "audio/wav" | "audio/ogg" | "audio/mp4" | "mp3" | "wav"
        | "ogg" | "m4a" => Some(IconName::FileThumbAudio),
        "application/pdf" | "pdf" => Some(IconName::FileThumbPdf),
        "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "doc"
        | "docx"
        | "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "ppt"
        | "pptx"
        | "text/markdown"
        | "md" => Some(IconName::FileThumbDoc),
        "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "xls"
        | "xlsx"
        | "text/csv"
        | "application/csv"
        | "csv" => Some(IconName::FileThumbXls),
        "application/zip"
        | "application/vnd.rar"
        | "application/x-compressed"
        | "application/x-7z-compressed"
        | "application/x-zip-compressed"
        | "application/x-rar-compressed"
        | "zip"
        | "rar"
        | "7z" => Some(IconName::FileThumbRar),
        _ => None,
    };
    if let Some(icon) = from_mime {
        return icon;
    }
    icon_from_filename(filename).unwrap_or(IconName::FileThumbEmpty)
}

fn icon_from_filename(filename: &str) -> Option<IconName> {
    let name = filename.trim();
    let ext = name.rsplit_once('.')?.1;
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => IconName::FileThumbPdf,
        "doc" | "docx" | "odt" | "rtf" | "md" | "txt" | "ppt" | "pptx" => IconName::FileThumbDoc,
        "xls" | "xlsx" | "csv" | "ods" => IconName::FileThumbXls,
        "zip" | "rar" | "7z" | "gz" | "tar" => IconName::FileThumbRar,
        "json" | "js" | "ts" => IconName::FileThumbJson,
        "html" | "css" | "xml" | "svg" => IconName::FileThumbCode,
        "mp3" | "wav" | "ogg" | "m4a" => IconName::FileThumbAudio,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_document_types() {
        assert!(file_type_icon("application/pdf") == IconName::FileThumbPdf);
        assert!(file_type_icon("text/csv") == IconName::FileThumbXls);
        assert!(file_type_icon("text/markdown") == IconName::FileThumbDoc);
        assert!(file_type_icon("application/zip") == IconName::FileThumbRar);
        assert!(file_type_icon("application/json") == IconName::FileThumbJson);
        assert!(file_type_icon("text/html") == IconName::FileThumbCode);
        assert!(file_type_icon("audio/mpeg") == IconName::FileThumbAudio);
        assert!(file_type_icon("FILE") == IconName::FileThumbEmpty);
        assert!(file_type_icon("unknown/xyz") == IconName::FileThumbEmpty);
        assert!(file_type_icon("pdf") == IconName::FileThumbPdf);
        assert!(file_type_icon("docx") == IconName::FileThumbDoc);
    }

    #[test]
    fn falls_back_to_filename_extension() {
        assert!(file_type_icon_for("FILE", "report.pdf") == IconName::FileThumbPdf);
        assert!(file_type_icon_for("", "sheet.csv") == IconName::FileThumbXls);
        assert!(file_type_icon_for("FILE", "archive.zip") == IconName::FileThumbRar);
        assert!(file_type_icon_for("application/pdf", "x.csv") == IconName::FileThumbPdf);
        assert!(file_type_icon_for("FILE", "Mezon AI Summary.pdf") == IconName::FileThumbPdf);
    }
}
