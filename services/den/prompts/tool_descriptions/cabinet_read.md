Read a Cabinet page or a bounded text/JSON slice of one of its shared attachments.

A page read takes cabinet_ref and optionally version_ref; it returns immutable page content, source links, and the attachments currently readable by this Bear. Attachments are current page links, not part of the historical page revision. Private files and files belonging to another Bear are omitted.

An attachment read takes cabinet_ref and attachment_ref from that page's attachment list. offset_chars defaults to 0; limit_chars defaults to 12000 and is limited to 24000. Results include the text, total character count and next_offset_chars for another slice. Attachment reads cannot be combined with version_ref. Only UTF-8 text and JSON are supported; PDFs, images and other binary formats have no text extraction here. Both page and file access are rechecked on every read. No object key or signed storage URL is returned.
