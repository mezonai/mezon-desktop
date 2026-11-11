const MAX_HTML_BYTES: usize = 2 * 1024 * 1024;
const MAX_DEPTH: usize = 256;

pub(crate) fn clipboard_html_to_markdown(html: &str) -> Option<String> {
    if html.is_empty() || html.len() > MAX_HTML_BYTES {
        return None;
    }

    let fragment = extract_cf_html(html);
    if fragment.is_empty() {
        return None;
    }

    let mut builder = Builder::new();
    tokenize(fragment, &mut builder);
    builder.finish()
}

fn extract_cf_html(html: &str) -> &str {
    if !html.starts_with("Version:") {
        return html;
    }
    let header_end = html.find('<').unwrap_or(html.len());
    let header = &html[..header_end];

    slice_from_offsets(
        html,
        cf_html_offset(header, "StartFragment:"),
        cf_html_offset(header, "EndFragment:"),
    )
    .or_else(|| {
        slice_from_offsets(
            html,
            cf_html_offset(header, "StartHTML:"),
            cf_html_offset(header, "EndHTML:"),
        )
    })
    .unwrap_or_else(|| html.get(header_end..).unwrap_or(""))
}

fn cf_html_offset(header: &str, key: &str) -> Option<usize> {
    let mut rest = header;
    while let Some(pos) = rest.find(key) {
        let at_boundary = pos == 0 || rest.as_bytes()[pos - 1].is_ascii_whitespace();
        rest = &rest[pos + key.len()..];
        if at_boundary {
            let token = rest
                .trim_start_matches([' ', '\t'])
                .split_ascii_whitespace()
                .next()?;
            return token.parse::<usize>().ok();
        }
    }
    None
}

fn slice_from_offsets(html: &str, start: Option<usize>, end: Option<usize>) -> Option<&str> {
    let (start, end) = (start?, end?);
    if start > 0
        && start <= end
        && end <= html.len()
        && html.is_char_boundary(start)
        && html.is_char_boundary(end)
    {
        Some(&html[start..end])
    } else {
        None
    }
}

fn decode_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        let Some(amp) = rest.find('&') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..amp]);
        pos += amp;
        if let Some((decoded, consumed)) = decode_entity_at(&input[pos..]) {
            out.push(decoded);
            pos += consumed;
        } else {
            out.push('&');
            pos += 1;
        }
    }
    out
}

fn decode_entity_at(s: &str) -> Option<(char, usize)> {
    let after_amp = s.strip_prefix('&')?;
    let bytes = after_amp.as_bytes();
    let mut len = 0;
    while len < bytes.len() && len <= 16 {
        let b = bytes[len];
        if b == b';' {
            let body = &after_amp[..len];
            let ch = resolve_entity(body)?;
            return Some((ch, 1 + len + 1));
        }
        if !(b.is_ascii_alphanumeric() || (len == 0 && b == b'#')) {
            return None;
        }
        len += 1;
    }
    None
}

fn resolve_entity(body: &str) -> Option<char> {
    if let Some(num) = body.strip_prefix('#') {
        let code = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            u32::from_str_radix(hex, 16).ok()?
        } else {
            if num.is_empty() || !num.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            num.parse::<u32>().ok()?
        };
        if code == 0 {
            return None;
        }
        return char::from_u32(code);
    }

    match body.to_ascii_lowercase().as_str() {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "ndash" => Some('\u{2013}'),
        "mdash" => Some('\u{2014}'),
        "hellip" => Some('\u{2026}'),
        "lsquo" => Some('\u{2018}'),
        "rsquo" => Some('\u{2019}'),
        "ldquo" => Some('\u{201C}'),
        "rdquo" => Some('\u{201D}'),
        "bull" => Some('\u{2022}'),
        "copy" => Some('\u{00A9}'),
        "reg" => Some('\u{00AE}'),
        "trade" => Some('\u{2122}'),
        "euro" => Some('\u{20AC}'),
        _ => None,
    }
}

struct StartTag {
    name: String,
    href: Option<String>,
    style: Option<String>,
    start: Option<String>,
    self_closing: bool,
}

fn tokenize(input: &str, builder: &mut Builder) {
    let mut pos = 0;
    while pos < input.len() {
        let rest = &input[pos..];
        if !rest.starts_with('<') {
            let end = rest.find('<').unwrap_or(rest.len());
            builder.on_text(&rest[..end]);
            pos += end;
            continue;
        }

        if let Some(after) = rest.strip_prefix("<!--") {
            if let Some(end) = after.find("-->") {
                pos += 4 + end + 3;
            } else {
                pos = input.len();
            }
            continue;
        }

        if let Some(after) = rest.strip_prefix("<!") {
            if let Some(end) = after.find('>') {
                pos += 2 + end + 1;
            } else {
                pos = input.len();
            }
            continue;
        }

        if let Some(after) = rest.strip_prefix("<?") {
            if let Some(end) = after.find('>') {
                pos += 2 + end + 1;
            } else {
                pos = input.len();
            }
            continue;
        }

        if let Some(after) = rest.strip_prefix("</") {
            match after.as_bytes().first().copied() {
                None => {
                    pos = input.len();
                }
                Some(b'>') => {
                    pos += 3;
                }
                Some(b) if b.is_ascii_alphabetic() => {
                    if let Some(end) = after.find('>') {
                        let inside = &after[..end];
                        let name_end = inside
                            .find(|c: char| c.is_ascii_whitespace() || c == '/')
                            .unwrap_or(inside.len());
                        let name = inside[..name_end].to_ascii_lowercase();
                        pos += 2 + end + 1;
                        builder.on_end_tag(&name);
                    } else {
                        pos = input.len();
                    }
                }
                Some(_) => {
                    if let Some(end) = after.find('>') {
                        pos += 2 + end + 1;
                    } else {
                        pos = input.len();
                    }
                }
            }
            continue;
        }

        if rest.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic) {
            if let Some((tag, consumed)) = parse_start_tag(&rest[1..]) {
                pos += 1 + consumed;
                let is_raw_text =
                    !tag.self_closing && (tag.name == "script" || tag.name == "style");
                let raw_tag_name = if is_raw_text {
                    Some(tag.name.clone())
                } else {
                    None
                };
                builder.on_start_tag(tag);
                if let Some(raw_name) = raw_tag_name {
                    pos += skip_raw_text_element(&input[pos..], &raw_name);
                    builder.on_end_tag(&raw_name);
                }
            } else {
                pos = input.len();
            }
            continue;
        }

        builder.on_text("<");
        pos += 1;
    }
}

fn skip_raw_text_element(s: &str, tag_name: &str) -> usize {
    let bytes = s.as_bytes();
    let tag_bytes = tag_name.as_bytes();
    let mut i = 0;
    while i + 2 + tag_bytes.len() <= bytes.len() {
        if bytes[i] == b'<'
            && bytes[i + 1] == b'/'
            && bytes[i + 2..i + 2 + tag_bytes.len()].eq_ignore_ascii_case(tag_bytes)
        {
            let after_idx = i + 2 + tag_bytes.len();
            if let Some(&next) = bytes.get(after_idx)
                && (next.is_ascii_whitespace() || next == b'/' || next == b'>')
            {
                if let Some(gt) = s[after_idx..].find('>') {
                    return after_idx + gt + 1;
                }
                return s.len();
            }
        }
        i += 1;
    }
    s.len()
}

fn parse_start_tag(after_lt: &str) -> Option<(StartTag, usize)> {
    let bytes = after_lt.as_bytes();
    let name_len = bytes
        .iter()
        .take_while(|b| !b.is_ascii_whitespace() && **b != b'/' && **b != b'>')
        .count();
    let name = after_lt[..name_len].to_ascii_lowercase();
    let mut k = name_len;
    let mut href = None;
    let mut style = None;
    let mut start = None;
    let mut self_closing = false;

    loop {
        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }
        if k >= bytes.len() {
            return None;
        }
        if bytes[k] == b'>' {
            k += 1;
            break;
        }
        if bytes[k] == b'/' {
            k += 1;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bytes.len() && bytes[k] == b'>' {
                self_closing = true;
                k += 1;
                break;
            }
            continue;
        }

        let attr_start = k;
        while k < bytes.len()
            && !bytes[k].is_ascii_whitespace()
            && bytes[k] != b'='
            && bytes[k] != b'>'
            && bytes[k] != b'/'
        {
            k += 1;
        }
        if k == attr_start {
            k += 1;
            continue;
        }
        let attr_name = &after_lt[attr_start..k];

        while k < bytes.len() && bytes[k].is_ascii_whitespace() {
            k += 1;
        }

        let attr_val = if k < bytes.len() && bytes[k] == b'=' {
            k += 1;
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < bytes.len() && (bytes[k] == b'"' || bytes[k] == b'\'') {
                let quote = bytes[k];
                k += 1;
                let val_start = k;
                while k < bytes.len() && bytes[k] != quote {
                    k += 1;
                }
                let val = &after_lt[val_start..k];
                if k < bytes.len() {
                    k += 1;
                }
                val
            } else {
                let val_start = k;
                while k < bytes.len() && !bytes[k].is_ascii_whitespace() && bytes[k] != b'>' {
                    k += 1;
                }
                &after_lt[val_start..k]
            }
        } else {
            ""
        };

        if attr_name.eq_ignore_ascii_case("href") && href.is_none() {
            href = Some(decode_entities(attr_val));
        } else if attr_name.eq_ignore_ascii_case("style") && style.is_none() {
            style = Some(decode_entities(attr_val));
        } else if attr_name.eq_ignore_ascii_case("start") && start.is_none() {
            start = Some(decode_entities(attr_val));
        }
    }

    Some((
        StartTag {
            name,
            href,
            style,
            start,
            self_closing,
        },
        k,
    ))
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_dropped_tag(tag: &str) -> bool {
    matches!(
        tag,
        "head" | "style" | "script" | "title" | "template" | "noscript" | "xml"
    )
}

fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "div"
            | "li"
            | "ul"
            | "ol"
            | "blockquote"
            | "tr"
            | "table"
            | "pre"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
    )
}

fn is_bold_tag(tag: &str) -> bool {
    matches!(
        tag,
        "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6"
    )
}

fn style_font_weight(style: Option<&str>) -> Option<bool> {
    let style = style?;
    let mut result = None;
    for decl in style.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        if !prop.trim().eq_ignore_ascii_case("font-weight") {
            continue;
        }
        let val = strip_important(val.trim()).trim();
        let Some(token) = val.split_ascii_whitespace().next() else {
            continue;
        };
        if token.eq_ignore_ascii_case("bold") || token.eq_ignore_ascii_case("bolder") {
            result = Some(true);
        } else if token.eq_ignore_ascii_case("normal") || token.eq_ignore_ascii_case("lighter") {
            result = Some(false);
        } else if let Ok(weight) = token.parse::<u16>() {
            result = Some((600..=900).contains(&weight));
        }
    }
    result
}

fn strip_important(val: &str) -> &str {
    match val.len().checked_sub(10) {
        Some(at)
            if val
                .get(at..)
                .is_some_and(|tail| tail.eq_ignore_ascii_case("!important")) =>
        {
            &val[..at]
        }
        _ => val,
    }
}

fn style_is_monospace(style: Option<&str>) -> bool {
    style.is_some_and(|style| {
        style.split(';').any(|decl| {
            decl.split_once(':').is_some_and(|(prop, val)| {
                prop.trim().eq_ignore_ascii_case("font-family")
                    && val.to_ascii_lowercase().contains("monospace")
            })
        })
    })
}

fn style_keep_newlines(style: Option<&str>) -> Option<bool> {
    let style = style?;
    let mut result = None;
    for decl in style.split(';') {
        let Some((prop, val)) = decl.split_once(':') else {
            continue;
        };
        let prop = prop.trim();
        let val = strip_important(val.trim()).trim();
        let Some(token) = val.split_ascii_whitespace().next() else {
            continue;
        };
        if prop.eq_ignore_ascii_case("white-space") {
            if token.eq_ignore_ascii_case("pre")
                || token.eq_ignore_ascii_case("pre-wrap")
                || token.eq_ignore_ascii_case("pre-line")
                || token.eq_ignore_ascii_case("break-spaces")
            {
                result = Some(true);
            } else if token.eq_ignore_ascii_case("normal") || token.eq_ignore_ascii_case("nowrap") {
                result = Some(false);
            }
        } else if prop.eq_ignore_ascii_case("white-space-collapse") {
            if token.eq_ignore_ascii_case("preserve")
                || token.eq_ignore_ascii_case("preserve-breaks")
                || token.eq_ignore_ascii_case("break-spaces")
            {
                result = Some(true);
            } else if token.eq_ignore_ascii_case("collapse")
                || token.eq_ignore_ascii_case("preserve-spaces")
            {
                result = Some(false);
            }
        }
    }
    result
}

fn is_code_tag(tag: &str) -> bool {
    matches!(tag, "pre" | "code" | "kbd" | "samp" | "tt")
}

fn parse_http_url(href: Option<&str>) -> Option<String> {
    let trimmed = href?.trim();
    let rest = if trimmed.len() >= 8
        && trimmed.is_char_boundary(8)
        && trimmed[..8].eq_ignore_ascii_case("https://")
    {
        &trimmed[8..]
    } else if trimmed.len() >= 7
        && trimmed.is_char_boundary(7)
        && trimmed[..7].eq_ignore_ascii_case("http://")
    {
        &trimmed[7..]
    } else {
        return None;
    };
    if rest.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn same_url(label: &str, url: &str) -> bool {
    let trimmed = label.trim();
    let a = trimmed.strip_suffix('/').unwrap_or(trimmed);
    let b = url.strip_suffix('/').unwrap_or(url);
    if a == b {
        return true;
    }
    if a.len() != b.len() {
        return false;
    }
    a.find("://").is_some_and(|idx| {
        b.get(..idx)
            .is_some_and(|bs| a[..idx].eq_ignore_ascii_case(bs))
            && a[idx..] == b[idx..]
    })
}

enum ListKind {
    Ul,
    Ol(i64),
}

struct LinkState {
    url: Option<String>,
    label: String,
    line_start_bytes: Option<usize>,
}

struct OpenElem {
    tag: String,
    bold: bool,
    dropped: bool,
    in_pre: bool,
    keep_newlines: bool,
    code: bool,
    list: Option<ListKind>,
    link: Option<LinkState>,
    row_cells: Option<usize>,
}

struct Run {
    text: String,
    bold: bool,
}

#[derive(Default)]
struct LineBuf {
    runs: Vec<Run>,
    list_prefix: Option<String>,
    pending_space: bool,
    pending_space_bold: bool,
    suppress_space: bool,
    has_cells: bool,
}

struct Builder {
    stack: Vec<OpenElem>,
    lines: Vec<String>,
    line: LineBuf,
    pending_li_prefix: Option<String>,
    open_links: usize,
    after_link_suffix: bool,
    has_bold: bool,
    has_link_url: bool,
    has_list: bool,
    has_quote: bool,
}

impl Builder {
    fn new() -> Self {
        Self {
            stack: Vec::new(),
            lines: Vec::new(),
            line: LineBuf::default(),
            pending_li_prefix: None,
            open_links: 0,
            after_link_suffix: false,
            has_bold: false,
            has_link_url: false,
            has_list: false,
            has_quote: false,
        }
    }

    fn current_bold(&self) -> bool {
        self.stack.last().is_some_and(|e| e.bold)
    }

    fn current_dropped(&self) -> bool {
        self.stack.last().is_some_and(|e| e.dropped)
    }

    fn current_in_pre(&self) -> bool {
        self.stack.last().is_some_and(|e| e.in_pre)
    }

    fn current_keep_newlines(&self) -> bool {
        self.stack.last().is_some_and(|e| e.keep_newlines)
    }

    fn line_text_len(&self) -> usize {
        self.line.runs.iter().map(|r| r.text.len()).sum()
    }

    fn unbold_line_from(&mut self, start_byte: usize) {
        let mut offset = 0usize;
        let old_runs = std::mem::take(&mut self.line.runs);
        for run in old_runs {
            let run_end = offset + run.text.len();
            if run_end <= start_byte || !run.bold {
                self.push_run(&run.text, run.bold);
            } else if offset >= start_byte {
                if !run.text.trim().is_empty() {
                    self.has_link_url = true;
                }
                self.push_run(&run.text, false);
            } else if run.text.is_char_boundary(start_byte - offset) {
                let split_at = start_byte - offset;
                self.push_run(&run.text[..split_at], true);
                if !run.text[split_at..].trim().is_empty() {
                    self.has_link_url = true;
                }
                self.push_run(&run.text[split_at..], false);
            } else {
                self.push_run(&run.text, run.bold);
            }
            offset = run_end;
        }
    }

    fn quote_depth(&self) -> usize {
        self.stack.iter().filter(|e| e.tag == "blockquote").count()
    }

    fn next_list_prefix(&mut self) -> Option<String> {
        let list_depth = self.stack.iter().filter(|e| e.list.is_some()).count();
        if list_depth == 0 {
            return None;
        }
        let indent = "  ".repeat(list_depth - 1);
        match self.stack.iter_mut().rev().find_map(|e| e.list.as_mut())? {
            ListKind::Ul => Some(format!("{indent}- ")),
            ListKind::Ol(next) => {
                let n = *next;
                *next += 1;
                Some(format!("{indent}{n}. "))
            }
        }
    }

    fn line_has_content(&self) -> bool {
        !self.line.runs.is_empty() || self.line.has_cells
    }

    fn ensure_new_line(&mut self) {
        if self.line_has_content() {
            self.push_line();
        }
    }

    fn push_run(&mut self, text: &str, bold: bool) {
        if text.is_empty() {
            return;
        }
        if let Some(last) = self.line.runs.last_mut()
            && last.bold == bold
        {
            last.text.push_str(text);
        } else {
            self.line.runs.push(Run {
                text: text.to_string(),
                bold,
            });
        }
    }

    fn push_char(&mut self, ch: char, bold: bool) {
        if let Some(last) = self.line.runs.last_mut()
            && last.bold == bold
        {
            last.text.push(ch);
        } else {
            let mut text = String::with_capacity(ch.len_utf8());
            text.push(ch);
            self.line.runs.push(Run { text, bold });
        }
    }

    fn take_li_prefix_if_needed(&mut self) {
        if self.line.list_prefix.is_none()
            && let Some(prefix) = self.pending_li_prefix.take()
        {
            self.line.list_prefix = Some(prefix);
        }
    }

    fn append_link_label_char(&mut self, ch: char) {
        if self.open_links == 0 {
            return;
        }
        if let Some(link) = self.stack.iter_mut().rev().find_map(|e| e.link.as_mut()) {
            link.label.push(ch);
        }
    }

    fn on_text(&mut self, raw: &str) {
        if self.current_dropped() || raw.is_empty() {
            return;
        }
        let decoded = decode_entities(raw);
        let bold = self.current_bold();

        if self.current_in_pre() {
            let mut chars = decoded.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\r' {
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    self.push_line();
                } else if ch == '\n' {
                    self.push_line();
                } else {
                    self.take_li_prefix_if_needed();
                    if std::mem::take(&mut self.after_link_suffix)
                        && !ch.is_whitespace()
                        && !matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')')
                    {
                        self.push_char(' ', false);
                    }
                    self.push_char(ch, bold);
                    self.append_link_label_char(ch);
                }
            }
            return;
        }

        let keep_newlines = self.current_keep_newlines();
        let mut chars = decoded.chars().peekable();
        while let Some(ch) = chars.next() {
            if keep_newlines && ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
                self.line.pending_space = false;
                self.push_line();
            } else if keep_newlines && ch == '\n' {
                self.line.pending_space = false;
                self.push_line();
            } else if ch.is_whitespace() {
                self.after_link_suffix = false;
                if self.line_has_content() && !self.line.suppress_space && !self.line.pending_space
                {
                    self.line.pending_space = true;
                    self.line.pending_space_bold = bold;
                }
                self.append_link_label_char(' ');
            } else {
                self.take_li_prefix_if_needed();
                if self.line.pending_space {
                    let space_bold = self.line.pending_space_bold;
                    self.line.pending_space = false;
                    self.after_link_suffix = false;
                    self.push_char(' ', space_bold);
                } else if std::mem::take(&mut self.after_link_suffix)
                    && !matches!(ch, '.' | ',' | ';' | ':' | '!' | '?' | ')')
                {
                    self.push_char(' ', false);
                }
                self.line.suppress_space = false;
                self.push_char(ch, bold);
                self.append_link_label_char(ch);
            }
        }
    }

    fn on_start_tag(&mut self, tag: StartTag) {
        let parent_dropped = self.current_dropped();
        let dropped = parent_dropped || is_dropped_tag(&tag.name);

        if !dropped {
            if tag.name == "br" {
                self.push_line();
                return;
            }
            if tag.name == "li" {
                let list_floor = self
                    .stack
                    .iter()
                    .rposition(|e| e.list.is_some())
                    .map_or(0, |idx| idx + 1);
                if let Some(rel) = self.stack[list_floor..].iter().rposition(|e| e.tag == "li") {
                    self.pop_down_to(list_floor + rel);
                }
            }
            if is_block_tag(&tag.name) {
                self.ensure_new_line();
            }
            if tag.name == "ul" || tag.name == "ol" {
                self.pending_li_prefix = None;
            }
            if tag.name == "li" {
                self.pending_li_prefix = self.next_list_prefix();
            }
            if (tag.name == "td" || tag.name == "th")
                && let Some(cells) = self
                    .stack
                    .iter_mut()
                    .rev()
                    .find_map(|e| e.row_cells.as_mut())
            {
                let index = *cells;
                *cells += 1;
                if index > 0 {
                    self.line.pending_space = false;
                    self.after_link_suffix = false;
                    self.push_char('\t', false);
                    self.line.suppress_space = true;
                }
                self.line.has_cells = true;
            }
        }

        if tag.self_closing || is_void_tag(&tag.name) || self.stack.len() >= MAX_DEPTH {
            return;
        }

        let parent_bold = self.current_bold();
        let parent_in_pre = self.current_in_pre();
        let parent_keep_newlines = self.current_keep_newlines();
        let code = self.stack.last().is_some_and(|e| e.code)
            || is_code_tag(&tag.name)
            || style_is_monospace(tag.style.as_deref());
        let bold = !code
            && style_font_weight(tag.style.as_deref())
                .unwrap_or_else(|| is_bold_tag(&tag.name) || parent_bold);
        let in_pre = parent_in_pre || tag.name == "pre";
        let keep_newlines =
            style_keep_newlines(tag.style.as_deref()).unwrap_or(parent_keep_newlines);
        let list = if dropped {
            None
        } else {
            match tag.name.as_str() {
                "ul" => Some(ListKind::Ul),
                "ol" => {
                    let first = tag
                        .start
                        .as_deref()
                        .and_then(|s| s.trim().parse::<i64>().ok())
                        .unwrap_or(1);
                    Some(ListKind::Ol(first))
                }
                _ => None,
            }
        };
        let link = if !dropped && tag.name == "a" {
            self.open_links += 1;
            Some(LinkState {
                url: parse_http_url(tag.href.as_deref()),
                label: String::new(),
                line_start_bytes: Some(self.line_text_len()),
            })
        } else {
            None
        };
        let row_cells = if !dropped && tag.name == "tr" {
            Some(0)
        } else {
            None
        };

        self.stack.push(OpenElem {
            tag: tag.name,
            bold,
            dropped,
            in_pre,
            keep_newlines,
            code,
            list,
            link,
            row_cells,
        });
    }

    fn on_end_tag(&mut self, name: &str) {
        if let Some(idx) = self.stack.iter().rposition(|e| e.tag == name) {
            self.pop_down_to(idx);
        }
    }

    fn pop_down_to(&mut self, target_idx: usize) {
        while self.stack.len() > target_idx {
            let is_block = self
                .stack
                .last()
                .is_some_and(|e| !e.dropped && is_block_tag(&e.tag));
            if is_block {
                self.ensure_new_line();
            }
            let Some(elem) = self.stack.pop() else {
                break;
            };
            if !elem.dropped {
                if let Some(link) = elem.link {
                    self.open_links -= 1;
                    self.finish_link(link);
                }
                if elem.tag == "li" {
                    self.pending_li_prefix = None;
                }
            }
        }
    }

    fn finish_link(&mut self, link: LinkState) {
        let Some(url) = link.url else {
            return;
        };
        let label = link.label.trim();
        if label.is_empty() {
            return;
        }
        if same_url(label, &url) {
            if let Some(start_byte) = link.line_start_bytes {
                self.unbold_line_from(start_byte);
            }
            return;
        }
        let had_trailing_space = self.line.pending_space;
        self.line.pending_space = false;
        self.line.suppress_space = false;
        let suffix = format!(" ({url})");
        if self.line_has_content() {
            self.push_run(&suffix, false);
            self.after_link_suffix = true;
        } else if let Some(prev) = self.lines.last_mut().filter(|l| !l.is_empty()) {
            prev.push_str(&suffix);
            self.after_link_suffix = true;
        } else {
            self.push_run(&url, false);
        }
        if had_trailing_space && self.line_has_content() {
            self.line.pending_space = true;
            self.line.pending_space_bold = false;
        }
        self.has_link_url = true;
    }

    fn push_line(&mut self) {
        let in_pre = self.current_in_pre();
        let quote_depth = self.quote_depth();
        let mut line = std::mem::take(&mut self.line);
        self.after_link_suffix = false;
        for elem in &mut self.stack {
            if let Some(link) = elem.link.as_mut() {
                link.line_start_bytes = None;
            }
        }

        if !in_pre {
            while let Some(last) = line.runs.last_mut() {
                let trimmed_len = last.text.trim_end_matches(' ').len();
                last.text.truncate(trimmed_len);
                if last.text.is_empty() {
                    line.runs.pop();
                } else {
                    break;
                }
            }
        }

        let mut merged: Vec<Run> = Vec::with_capacity(line.runs.len());
        for run in line.runs {
            if run.text.is_empty() {
                continue;
            }
            if let Some(prev) = merged.last_mut()
                && prev.bold == run.bold
            {
                prev.text.push_str(&run.text);
            } else {
                merged.push(run);
            }
        }

        let mut body = String::new();
        for run in merged {
            if !run.bold {
                body.push_str(&run.text);
                continue;
            }
            let trimmed_start = run.text.trim_start_matches(char::is_whitespace);
            if trimmed_start.is_empty() {
                body.push_str(&run.text);
                continue;
            }
            let leading = run.text.len() - trimmed_start.len();
            let core = trimmed_start.trim_end_matches(char::is_whitespace);
            body.push_str(&run.text[..leading]);
            body.push_str("**");
            body.push_str(core);
            body.push_str("**");
            body.push_str(&trimmed_start[core.len()..]);
            self.has_bold = true;
        }

        if body.is_empty() {
            self.lines.push(String::new());
            return;
        }

        let mut full = String::new();
        if quote_depth > 0 {
            for _ in 0..quote_depth {
                full.push_str("> ");
            }
            self.has_quote = true;
        }
        if let Some(list_prefix) = line.list_prefix {
            full.push_str(&list_prefix);
            self.has_list = true;
        }
        full.push_str(&body);
        self.lines.push(full);
    }

    fn finish(mut self) -> Option<String> {
        self.pop_down_to(0);
        self.ensure_new_line();

        if !(self.has_bold || self.has_link_url || self.has_list || self.has_quote) {
            return None;
        }

        let first = self.lines.iter().position(|l| !l.trim().is_empty())?;
        let last = self.lines.iter().rposition(|l| !l.trim().is_empty())?;
        let joined = self.lines[first..=last].join("\n");
        let collapsed = collapse_newlines(joined);
        if collapsed.is_empty() {
            None
        } else {
            Some(collapsed)
        }
    }
}

fn collapse_newlines(input: String) -> String {
    if !input.contains("\n\n\n") {
        return input;
    }
    let mut out = String::with_capacity(input.len());
    let mut count = 0usize;
    for ch in input.chars() {
        if ch == '\n' {
            count += 1;
            if count <= 2 {
                out.push('\n');
            }
        } else {
            count = 0;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{MAX_HTML_BYTES, clipboard_html_to_markdown};

    fn build_cf_html(prefix_html: &str, fragment: &str, suffix_html: &str) -> String {
        let header_placeholder = concat!(
            "Version:0.9\r\n",
            "StartHTML:0000000000\r\n",
            "EndHTML:0000000000\r\n",
            "StartFragment:0000000000\r\n",
            "EndFragment:0000000000\r\n"
        );
        let start_frag_marker = "<!--StartFragment-->";
        let end_frag_marker = "<!--EndFragment-->";
        let start_html = header_placeholder.len();
        let start_fragment = start_html + prefix_html.len() + start_frag_marker.len();
        let end_fragment = start_fragment + fragment.len();
        let end_html = end_fragment + end_frag_marker.len() + suffix_html.len();
        format!(
            "Version:0.9\r\nStartHTML:{start_html:010}\r\nEndHTML:{end_html:010}\r\nStartFragment:{start_fragment:010}\r\nEndFragment:{end_fragment:010}\r\n{prefix_html}{start_frag_marker}{fragment}{end_frag_marker}{suffix_html}"
        )
    }

    #[test]
    fn gmail_rich_message_keeps_bold_links_lists_and_quotes() {
        let html = concat!(
            r#"<div dir="ltr">Hi <b>team</b>, please review <i>the</i> <u>updated</u> "#,
            r#"<a href="https://docs.mezon.vn/spec?v=2&amp;lang=vi">release spec</a>."#,
            r#"<div><br></div>"#,
            r#"<div>Checklist:</div>"#,
            r#"<div><ul>"#,
            r#"<li><b>Desktop</b> rich paste<br></li>"#,
            r#"<li>Homepage <a href="https://mezon.vn/">https://mezon.vn</a><br></li>"#,
            r#"</ul></div>"#,
            r#"<div class="gmail_quote"><blockquote class="gmail_quote" style="margin:0px 0px 0px 0.8ex;border-left:1px solid rgb(204,204,204);padding-left:1ex">"#,
            r#"<div>Ship by <b>Friday</b><br>Thanks!</div>"#,
            r#"</blockquote></div></div>"#
        );

        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some(
                "Hi **team**, please review the updated release spec (https://docs.mezon.vn/spec?v=2&lang=vi).\n\nChecklist:\n- **Desktop** rich paste\n- Homepage https://mezon.vn\n> Ship by **Friday**\n> Thanks!"
            )
        );
    }

    #[test]
    fn google_docs_normal_bold_wrapper_and_styled_spans_convert_accurately() {
        let html = concat!(
            r#"<meta charset="utf-8">"#,
            r#"<b style="font-weight:normal;" id="docs-internal-guid-7f3a1c20-7fff-4b11-9e12-001122334455">"#,
            r#"<p dir="ltr" style="line-height:1.38;margin-top:0pt;margin-bottom:0pt;">"#,
            r#"<span style="font-size:11pt;font-family:Arial,sans-serif;font-weight:700;font-style:normal;">Sprint Goal</span>"#,
            r#"<span style="font-size:11pt;font-family:Arial,sans-serif;font-weight:400;font-style:italic;"> for desktop</span>"#,
            r#"</p>"#,
            r#"<ul style="margin-top:0;margin-bottom:0;">"#,
            r#"<li dir="ltr" style="list-style-type:disc;"><p dir="ltr"><span style="font-weight:400;">Keep </span><span style="font-weight:700;">bold</span><span style="font-weight:400;"> text</span></p></li>"#,
            r#"<li dir="ltr" style="list-style-type:disc;"><p dir="ltr"><a href="https://mezon.vn/guide" style="text-decoration:none;"><span style="font-weight:400;text-decoration:underline;">Read guide</span></a></p></li>"#,
            r#"</ul>"#,
            r#"<p dir="ltr"><b>Bold <span style="font-weight:400;">plain</span> again</b></p>"#,
            r#"</b>"#
        );

        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some(
                "**Sprint Goal** for desktop\n- Keep **bold** text\n- Read guide (https://mezon.vn/guide)\n**Bold** plain **again**"
            )
        );
    }

    #[test]
    fn word_cf_html_slices_fragment_and_drops_mso_noise() {
        let prefix = concat!(
            "<html xmlns:o=\"urn:schemas-microsoft-com:office:office\" xmlns:w=\"urn:schemas-microsoft-com:office:word\">",
            "<head>",
            "<!--[if gte mso 9]><xml><w:WordDocument><w:View>Normal</w:View></w:WordDocument></xml><![endif]-->",
            "<style>p.MsoNormal, li.MsoNormal { margin: 0in; font-size: 11.0pt; }</style>",
            "</head><body lang=EN-US>"
        );
        let fragment = concat!(
            "<p class=MsoNormal><b style='mso-bidi-font-weight:normal'><span lang=EN-US>Weekly Sync</span></b><o:p></o:p></p>",
            "<p class=MsoNormal><o:p>&nbsp;</o:p></p>",
            "<ul style='margin-top:0in' type=disc>",
            "<li class=MsoNormal style='mso-list:l0 level1 lfo1'><!--[if !supportLists]><span style='mso-list:Ignore'>·&nbsp;</span><![endif]--><span lang=EN-US>Ship <b>rich paste</b></span><o:p>&nbsp;</o:p></li>",
            "<li class=MsoNormal style='mso-list:l0 level1 lfo1'><![if !supportLists]><![endif]><span lang=EN-US>Check <a href=\"https://mezon.vn/tasks\">board</a></span><o:p></o:p></li>",
            "</ul>"
        );
        let suffix = "</body></html>";
        let cf_html = build_cf_html(prefix, fragment, suffix);

        assert_eq!(
            clipboard_html_to_markdown(&cf_html).as_deref(),
            Some("**Weekly Sync**\n- Ship **rich paste**\n- Check board (https://mezon.vn/tasks)")
        );
    }

    #[test]
    fn cf_html_uses_utf8_byte_offsets_after_vietnamese_prefix() {
        let prefix = "<html><head><title>Báo cáo tiến độ dự án — Nguyễn Văn Ánh</title></head><body><p>Đoạn ngoài vùng chọn</p>";
        let fragment =
            "<p>Xin chào <b>Việt Nam</b> tại <a href=\"https://mezon.vn/vi\">Mezon</a>!</p>";
        let suffix = "<p>Đoạn cuối bị bỏ qua</p></body></html>";
        let cf_html = build_cf_html(prefix, fragment, suffix);

        assert!(prefix.len() > prefix.chars().count());
        assert_eq!(
            clipboard_html_to_markdown(&cf_html).as_deref(),
            Some("Xin chào **Việt Nam** tại Mezon (https://mezon.vn/vi)!")
        );
    }

    #[test]
    fn malformed_html_handles_unclosed_bold_stray_end_tags_and_bare_anchor() {
        let html = "<div>stray </i></u> tags and <a>anchor without href</a> then <b>unclosed bold";
        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some("stray tags and anchor without href then **unclosed bold**")
        );
    }

    #[test]
    fn unsafe_schemes_and_scripts_keep_only_safe_text() {
        let unformatted = r#"<script>alert("<b>xss</b>")</script><img src=x onerror="alert(1)"><a href="javascript:alert(1)">Click me</a>"#;
        assert_eq!(clipboard_html_to_markdown(unformatted), None);

        let mixed = concat!(
            r#"<p><b>Notice:</b> <script>const s = "<b>ignored</b>";</script>"#,
            r#"<img src="x" onerror="evil()">"#,
            r#"<a href="javascript:alert(1)">JS</a>, "#,
            r#"<a href="data:text/html,<b>x</b>">Data</a>, "#,
            r#"<a href="mailto:dev@mezon.vn">Mail</a>, "#,
            r#"<a href="file:///etc/passwd">File</a>, "#,
            r#"<a href="  HTTPS://mezon.vn/security  ">Policy</a></p>"#
        );
        assert_eq!(
            clipboard_html_to_markdown(mixed).as_deref(),
            Some("**Notice:** JS, Data, Mail, File, Policy (HTTPS://mezon.vn/security)")
        );
    }

    #[test]
    fn decodes_named_numeric_hex_and_preserves_unknown_entities() {
        let html = concat!(
            "<p><b>Entities:</b> &amp; &lt; &gt; &quot; &apos; &#39; a&nbsp;b ",
            "&ndash; &mdash; &hellip; &lsquo;hi&rsquo; &ldquo;yo&rdquo; ",
            "&bull; &copy; &reg; &trade; &euro; &#8363; &#x1F680; &notreal;</p>"
        );
        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some("**Entities:** & < > \" ' ' a b – — … ‘hi’ “yo” • © ® ™ € ₫ 🚀 &notreal;")
        );
    }

    #[test]
    fn nested_lists_and_ordered_list_start_attribute() {
        let html = concat!(
            "<ol start=\"3\">",
            "<li>Third<ul><li>Sub one</li><li>Sub two<ol start=10><li>Ten</li></ol></li></ul></li>",
            "<li>Fourth</li>",
            "</ol>"
        );
        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some("3. Third\n  - Sub one\n  - Sub two\n    10. Ten\n4. Fourth")
        );
    }

    #[test]
    fn bold_moves_surrounding_spaces_outside_markers_and_never_crosses_lines() {
        assert_eq!(
            clipboard_html_to_markdown("<b>bold </b>next").as_deref(),
            Some("**bold** next")
        );
        assert_eq!(
            clipboard_html_to_markdown("prev<b> bold</b>").as_deref(),
            Some("prev **bold**")
        );
        assert_eq!(
            clipboard_html_to_markdown("a<b>   </b>b <b>ok</b>").as_deref(),
            Some("a b **ok**")
        );
        assert_eq!(
            clipboard_html_to_markdown("<b>line one<br>line two</b>").as_deref(),
            Some("**line one**\n**line two**")
        );
    }

    #[test]
    fn plain_only_html_returns_none() {
        assert_eq!(clipboard_html_to_markdown(""), None);
        assert_eq!(clipboard_html_to_markdown("<span>just text</span>"), None);
        assert_eq!(
            clipboard_html_to_markdown("<p><i>italic</i> <u>underline</u> <del>strike</del></p>"),
            None
        );
        assert_eq!(
            clipboard_html_to_markdown("<a href=\"https://mezon.vn/\">https://mezon.vn</a>"),
            None
        );
    }

    #[test]
    fn html_over_two_mib_cap_returns_none() {
        let prefix = "<b>bold</b>";
        let over_cap = format!("{prefix}{}", "a".repeat(MAX_HTML_BYTES - prefix.len() + 1));
        assert_eq!(over_cap.len(), MAX_HTML_BYTES + 1);
        assert_eq!(clipboard_html_to_markdown(&over_cap), None);
    }

    #[test]
    fn tables_pre_blocks_empty_link_labels_and_blank_line_collapse() {
        let html = concat!(
            "<h2>Summary</h2>",
            "<br><br><br>",
            "<table><tr><th><b>Service</b></th><th>Status</th></tr><tr><td>API</td><td>OK</td></tr></table>",
            "<pre>fn main() {\n    println!(\"hi\");\n}</pre>",
            "<p>Link: <a href=\"https://mezon.vn/status\"></a></p>"
        );
        assert_eq!(
            clipboard_html_to_markdown(html).as_deref(),
            Some(
                "**Summary**\n\n**Service**\tStatus\nAPI\tOK\nfn main() {\n    println!(\"hi\");\n}\nLink:"
            )
        );
    }

    #[test]
    fn link_suffix_separates_following_text_while_keeping_punctuation_attached() {
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<p><a href="https://github.com/mezonai">Mezon</a>'s new <b>release</b></p>"#
            )
            .as_deref(),
            Some("Mezon (https://github.com/mezonai) 's new **release**")
        );
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<p>请看<a href="https://x.com/doc">文档</a>了解<b>更多</b></p>"#
            )
            .as_deref(),
            Some("请看文档 (https://x.com/doc) 了解**更多**")
        );
        assert_eq!(
            clipboard_html_to_markdown(r#"<p>Two <a href="https://x.com/pr">PR</a>s merged</p>"#)
                .as_deref(),
            Some("Two PR (https://x.com/pr) s merged")
        );
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<p>See <a href="https://en.wikipedia.org/wiki/Rust_(programming_language)">Rust</a>.</p>"#
            )
            .as_deref(),
            Some("See Rust (https://en.wikipedia.org/wiki/Rust_(programming_language)).")
        );
    }

    #[test]
    fn bold_url_is_unbolded_when_label_matches_href() {
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<b>Meeting: <a href="https://meet.google.com/abc">https://meet.google.com/abc</a></b>"#
            )
            .as_deref(),
            Some("**Meeting:** https://meet.google.com/abc")
        );
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<a href="https://mezon.vn/x"><span style="font-weight:700">https://mezon.vn/x</span></a>"#
            )
            .as_deref(),
            Some("https://mezon.vn/x")
        );
        assert_eq!(
            clipboard_html_to_markdown(r#"<b><a href="https://mezon.vn/docs">Docs</a></b>"#)
                .as_deref(),
            Some("**Docs** (https://mezon.vn/docs)")
        );
    }

    #[test]
    fn whitespace_preserving_styles_keep_newlines_until_overridden_by_normal() {
        assert_eq!(
            clipboard_html_to_markdown(
                "<span style=\"white-space: break-spaces;\"><b> Release notes </b>\nfix A\nfix B</span>"
            )
            .as_deref(),
            Some("**Release notes**\nfix A\nfix B")
        );
        assert_eq!(
            clipboard_html_to_markdown(
                "<div style=\"white-space: pre-wrap;\">Line one\nLine two <a href=\"https://t.co/abc\">example.com/post</a>\nLine three</div>"
            )
            .as_deref(),
            Some("Line one\nLine two example.com/post (https://t.co/abc)\nLine three")
        );
        assert_eq!(
            clipboard_html_to_markdown(
                "<div style=\"white-space: pre-wrap;\"><b>Header</b>\n<span style=\"white-space: normal;\">alpha\nbeta</span>\ngamma</div>"
            )
            .as_deref(),
            Some("**Header**\nalpha beta\ngamma")
        );
    }

    #[test]
    fn image_only_link_does_not_inject_href() {
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<p>Top stories today</p><a href="https://news.example.com/r?u=abc&amp;t=1"><img src="t.jpg"></a><p>Markets rally</p>"#
            ),
            None
        );
        assert_eq!(
            clipboard_html_to_markdown(
                r#"<p><a href="https://news.example.com/r?u=abc&amp;t=1"><img src="t.jpg"></a><b>Markets</b> rally</p>"#
            )
            .as_deref(),
            Some("**Markets** rally")
        );
    }

    #[test]
    fn code_editor_html_with_bold_tokens_falls_back_to_plain_text() {
        let vscode = concat!(
            r#"<meta charset='utf-8'><div style="color: #d4d4d4;background-color: #1e1e1e;font-family: Menlo, Monaco, 'Courier New', monospace;font-weight: normal;font-size: 12px;line-height: 18px;white-space: pre;">"#,
            r#"<div><span style="color: #569cd6;font-weight: bold;">fn</span><span style="color: #d4d4d4;"> </span><span style="color: #dcdcaa;">main</span><span style="color: #d4d4d4;">() {</span></div>"#,
            r#"<div><span style="color: #d4d4d4;">    </span><span style="color: #569cd6;font-weight: bold;">let</span><span style="color: #d4d4d4;"> x = 1;</span></div>"#,
            r#"<div><span style="color: #d4d4d4;">}</span></div></div>"#
        );
        assert_eq!(clipboard_html_to_markdown(vscode), None);
        assert_eq!(
            clipboard_html_to_markdown("<p><b>Run</b> <code><b>cargo</b> test</code></p>")
                .as_deref(),
            Some("**Run** cargo test")
        );
    }

    #[test]
    fn multibyte_style_values_and_deep_nesting_do_not_panic_or_hang() {
        assert_eq!(
            clipboard_html_to_markdown(r#"<span style="font-weight:7ữữữữữ">x</span><b>y</b>"#)
                .as_deref(),
            Some("x**y**")
        );
        assert_eq!(
            clipboard_html_to_markdown(r#"<span style="font-weight: 700 !IMPORTANT">heavy</span>"#)
                .as_deref(),
            Some("**heavy**")
        );
        let deep = format!(
            "<b>top</b>{}text{}",
            "<span>".repeat(150_000),
            "</div>".repeat(150_000)
        );
        let started = std::time::Instant::now();
        assert_eq!(
            clipboard_html_to_markdown(&deep).as_deref(),
            Some("**top**text")
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }
}
