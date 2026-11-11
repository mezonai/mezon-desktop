use crate::components::primitives::IconName;

pub fn file_type_icon(filetype: &str) -> IconName {
    file_type_icon_for(filetype, "")
}

pub fn file_type_icon_for(filetype: &str, filename: &str) -> IconName {
    let key = filetype.trim().to_ascii_lowercase();
    let from_mime = match key.as_str() {
        "text/plain" | "txt" => Some(IconName::FileText),
        "text/html" | "text/css" | "application/xml" | "image/svg+xml" | "html" | "css" | "xml" => {
            Some(IconName::FileCode)
        }
        "application/javascript" | "application/json" | "json" | "js" => Some(IconName::FileCode),
        "audio/mpeg" | "audio/mp3" | "audio/wav" | "audio/ogg" | "audio/mp4" | "mp3" | "wav"
        | "ogg" | "m4a" => Some(IconName::FileAudio),
        "application/pdf" | "pdf" => Some(IconName::FilePdf),
        "application/msword"
        | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        | "doc"
        | "docx"
        | "application/vnd.ms-powerpoint"
        | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
        | "ppt"
        | "pptx"
        | "text/markdown"
        | "md" => Some(IconName::FileText),
        "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        | "xls"
        | "xlsx"
        | "text/csv"
        | "application/csv"
        | "csv" => Some(IconName::FileSheet),
        "application/zip"
        | "application/vnd.rar"
        | "application/x-compressed"
        | "application/x-7z-compressed"
        | "application/x-zip-compressed"
        | "application/x-rar-compressed"
        | "zip"
        | "rar"
        | "7z" => Some(IconName::FileArchive),
        _ => None,
    };
    if let Some(icon) = from_mime {
        return icon;
    }
    icon_from_filename(filename).unwrap_or(IconName::File)
}

fn icon_from_filename(filename: &str) -> Option<IconName> {
    let name = filename.trim();
    let ext = name.rsplit_once('.')?.1;
    if ext.is_empty() || ext.contains('/') {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    Some(match ext.as_str() {
        "pdf" => IconName::FilePdf,
        "doc" | "docx" | "odt" | "rtf" | "md" | "txt" | "ppt" | "pptx" => IconName::FileText,
        "xls" | "xlsx" | "csv" | "ods" => IconName::FileSheet,
        "zip" | "rar" | "7z" | "gz" | "tar" => IconName::FileArchive,
        "json" | "js" | "ts" => IconName::FileCode,
        "html" | "css" | "xml" | "svg" => IconName::FileCode,
        "mp3" | "wav" | "ogg" | "m4a" => IconName::FileAudio,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_common_document_types() {
        assert!(file_type_icon("application/pdf") == IconName::FilePdf);
        assert!(file_type_icon("text/csv") == IconName::FileSheet);
        assert!(file_type_icon("text/markdown") == IconName::FileText);
        assert!(file_type_icon("application/zip") == IconName::FileArchive);
        assert!(file_type_icon("application/json") == IconName::FileCode);
        assert!(file_type_icon("text/html") == IconName::FileCode);
        assert!(file_type_icon("audio/mpeg") == IconName::FileAudio);
        assert!(file_type_icon("FILE") == IconName::File);
        assert!(file_type_icon("unknown/xyz") == IconName::File);
        assert!(file_type_icon("pdf") == IconName::FilePdf);
        assert!(file_type_icon("docx") == IconName::FileText);
    }

    #[test]
    fn falls_back_to_filename_extension() {
        assert!(file_type_icon_for("FILE", "report.pdf") == IconName::FilePdf);
        assert!(file_type_icon_for("", "sheet.csv") == IconName::FileSheet);
        assert!(file_type_icon_for("FILE", "archive.zip") == IconName::FileArchive);
        assert!(file_type_icon_for("application/pdf", "x.csv") == IconName::FilePdf);
        assert!(file_type_icon_for("FILE", "Mezon AI Summary.pdf") == IconName::FilePdf);
    }
}
