use super::memory_manager_head::append_markdown_section;

#[test]
fn append_markdown_section_seeds_empty_file() {
    let result = append_markdown_section("", "## Meta", "- [Meta](./meta/index.md)");
    assert_eq!(result, "## Meta\n\n- [Meta](./meta/index.md)\n");
}

#[test]
fn append_markdown_section_appends_after_existing_content() {
    let result = append_markdown_section(
        "# Workplaces\n\nThis index lists the Bear's registered Workplaces.\n",
        "## Meta",
        "- [Meta](./meta/index.md)",
    );
    assert_eq!(
        result,
        "# Workplaces\n\nThis index lists the Bear's registered Workplaces.\n\n## Meta\n\n- [Meta](./meta/index.md)\n"
    );
}
