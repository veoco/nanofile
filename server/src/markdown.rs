//! Markdown → sanitized HTML rendering for wiki pages.

/// Render CommonMark (with GFM tables / strikethrough / footnotes) to HTML and
/// sanitize the output so raw HTML in the source can't inject scripts.
///
/// `pulldown-cmark` does not sanitize inline HTML; `ammonia` strips scripts /
/// event handlers and constrains `href`/`src` to safe schemes.
pub fn render_markdown(content: &str) -> String {
    let options = pulldown_cmark::Options::all();
    let parser = pulldown_cmark::Parser::new_ext(content, options);
    let mut html = String::with_capacity(content.len() * 2);
    pulldown_cmark::html::push_html(&mut html, parser);

    let mut builder = ammonia::Builder::default();
    builder
        .link_rel(Some("noopener noreferrer"))
        .url_relative(ammonia::UrlRelative::PassThrough);
    builder.clean(&html).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_markdown_with_tables() {
        let html = render_markdown("| a | b |\n|---|---|\n| 1 | 2 |\n");
        assert!(html.contains("<table>"), "GFM tables should render: {html}");
        assert!(html.contains("<td>1</td>"));
    }

    #[test]
    fn strips_script_and_event_handlers() {
        let html = render_markdown(
            "hello <script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[js](javascript:alert(1))",
        );
        assert!(
            !html.contains("<script"),
            "script tag must be stripped: {html}"
        );
        assert!(
            !html.contains("onerror"),
            "event handlers must be stripped: {html}"
        );
        assert!(
            !html.contains("javascript:"),
            "javascript: hrefs must be stripped: {html}"
        );
        assert!(html.contains("hello"), "text content preserved");
    }
}
