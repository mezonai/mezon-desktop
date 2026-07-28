use serde_json::{Value, json};

pub fn is_quill_delta(value: &Value) -> bool {
    value
        .get("ops")
        .and_then(|v| v.as_array())
        .is_some_and(|ops| !ops.is_empty())
}

pub fn quill_delta_to_tiptap_json(delta: &Value) -> Value {
    let ops = delta.get("ops").and_then(|v| v.as_array()).cloned();
    let Some(ops) = ops else {
        return json!({ "type": "doc", "content": [{ "type": "paragraph" }] });
    };

    let mut doc_content: Vec<Value> = Vec::new();
    let mut current_paragraph: Vec<Value> = Vec::new();
    let mut current_list: Option<(String, Vec<Value>)> = None;

    let flush_paragraph =
        |doc_content: &mut Vec<Value>,
         current_paragraph: &mut Vec<Value>,
         current_list: &mut Option<(String, Vec<Value>)>| {
            if current_paragraph.is_empty() {
                return;
            }
            let paragraph = json!({
                "type": "paragraph",
                "content": std::mem::take(current_paragraph)
            });
            if let Some((_, items)) = current_list.as_mut() {
                items.push(json!({
                    "type": "listItem",
                    "content": [paragraph]
                }));
            } else {
                doc_content.push(paragraph);
            }
        };

    let flush_list = |doc_content: &mut Vec<Value>,
                      current_list: &mut Option<(String, Vec<Value>)>| {
        if let Some((list_type, items)) = current_list.take()
            && !items.is_empty()
        {
            doc_content.push(json!({ "type": list_type, "content": items }));
        }
    };

    for op in ops {
        if let Some(insert) = op.get("insert")
            && insert.is_object()
            && !insert.as_object().is_some_and(|o| o.is_empty())
        {
            if let Some(image) = insert.get("image").and_then(|v| v.as_str()) {
                if !image.is_empty() {
                    flush_paragraph(&mut doc_content, &mut current_paragraph, &mut current_list);
                    flush_list(&mut doc_content, &mut current_list);
                    doc_content.push(json!({
                        "type": "image",
                        "attrs": { "src": image }
                    }));
                }
                continue;
            }
            continue;
        }

        let Some(text) = op.get("insert").and_then(|v| v.as_str()) else {
            continue;
        };
        let attrs = op.get("attributes");

        if text == "\n" {
            if let Some(level) = attrs.and_then(|a| a.get("header")).and_then(|v| v.as_u64()) {
                if !current_paragraph.is_empty() {
                    flush_list(&mut doc_content, &mut current_list);
                    doc_content.push(json!({
                        "type": "heading",
                        "attrs": { "level": level },
                        "content": std::mem::take(&mut current_paragraph)
                    }));
                }
                continue;
            }

            if let Some(list) = attrs.and_then(|a| a.get("list")).and_then(|v| v.as_str()) {
                let list_type = if list == "ordered" {
                    "orderedList"
                } else {
                    "bulletList"
                };
                let needs_new_list = current_list
                    .as_ref()
                    .is_none_or(|(kind, _)| kind != list_type);
                if needs_new_list {
                    flush_list(&mut doc_content, &mut current_list);
                    current_list = Some((list_type.to_string(), Vec::new()));
                }
                flush_paragraph(&mut doc_content, &mut current_paragraph, &mut current_list);
                continue;
            }

            flush_paragraph(&mut doc_content, &mut current_paragraph, &mut current_list);
            flush_list(&mut doc_content, &mut current_list);
            continue;
        }

        let lines: Vec<&str> = text.split('\n').collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.is_empty() {
                let mut node = json!({ "type": "text", "text": line });
                let mut marks: Vec<Value> = Vec::new();
                if attrs.and_then(|a| a.get("bold")).and_then(|v| v.as_bool()) == Some(true) {
                    marks.push(json!({ "type": "bold" }));
                }
                if attrs
                    .and_then(|a| a.get("italic"))
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    marks.push(json!({ "type": "italic" }));
                }
                if attrs
                    .and_then(|a| a.get("underline"))
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    marks.push(json!({ "type": "underline" }));
                }
                if attrs
                    .and_then(|a| a.get("strike"))
                    .and_then(|v| v.as_bool())
                    == Some(true)
                {
                    marks.push(json!({ "type": "strike" }));
                }
                if let Some(href) = attrs.and_then(|a| a.get("link")).and_then(|v| v.as_str()) {
                    marks.push(json!({ "type": "link", "attrs": { "href": href } }));
                }
                if !marks.is_empty() {
                    node["marks"] = json!(marks);
                }
                current_paragraph.push(node);
            }
            if i + 1 < lines.len() {
                flush_paragraph(&mut doc_content, &mut current_paragraph, &mut current_list);
                flush_list(&mut doc_content, &mut current_list);
            }
        }
    }

    flush_paragraph(&mut doc_content, &mut current_paragraph, &mut current_list);
    flush_list(&mut doc_content, &mut current_list);

    if doc_content.is_empty() {
        doc_content.push(json!({ "type": "paragraph" }));
    }

    json!({ "type": "doc", "content": doc_content })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_quill_delta() {
        let raw = json!({"ops":[{"insert":"Hello\n"}]});
        assert!(is_quill_delta(&raw));
        assert!(!is_quill_delta(&json!({"type":"doc"})));
    }

    #[test]
    fn converts_plain_quill_paragraph() {
        let raw = json!({"ops":[{"insert":"Hello world\n"}]});
        let out = quill_delta_to_tiptap_json(&raw);
        assert_eq!(out["type"], "doc");
        assert_eq!(out["content"][0]["type"], "paragraph");
        assert_eq!(out["content"][0]["content"][0]["text"], "Hello world");
    }

    #[test]
    fn converts_quill_bold_and_newlines() {
        let raw = json!({
            "ops": [
                { "insert": "Line one" },
                { "insert": "\n", "attributes": { "header": 2 } },
                { "insert": "Bold", "attributes": { "bold": true } },
                { "insert": "\n" }
            ]
        });
        let out = quill_delta_to_tiptap_json(&raw);
        assert_eq!(out["content"][0]["type"], "heading");
        assert_eq!(out["content"][0]["content"][0]["text"], "Line one");
        assert_eq!(out["content"][1]["type"], "paragraph");
        assert_eq!(out["content"][1]["content"][0]["text"], "Bold");
    }

    #[test]
    fn converts_quill_image_and_trailing_text() {
        let raw = json!({
            "ops": [
                { "insert": "Item one" },
                { "insert": "\n", "attributes": { "list": "ordered" } },
                { "insert": { "image": "https://cdn.mezon.ai/clan/photo.png" } },
                { "insert": "After image\n" }
            ]
        });
        let out = quill_delta_to_tiptap_json(&raw);
        let content = out["content"].as_array().expect("content");
        assert_eq!(content[0]["type"], "orderedList");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(
            content[1]["attrs"]["src"],
            "https://cdn.mezon.ai/clan/photo.png"
        );
        assert_eq!(content[2]["type"], "paragraph");
        assert_eq!(content[2]["content"][0]["text"], "After image");
    }
}
