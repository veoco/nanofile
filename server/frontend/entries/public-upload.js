// public-upload — the public upload-link page (standalone layout, no common
// bundle). Migrated from the inline <script> block in
// templates/web/upload_link_view.html; driven by data-* event delegation.
(function () {
    var ctxEl = document.getElementById('public-upload-context');
    var UPLOAD_TOKEN = ctxEl ? (ctxEl.dataset.uploadToken || '') : '';
    var MAX_SIZE_MB = ctxEl ? parseInt(ctxEl.dataset.maxSizeMb || '0', 10) : 0;
    // User-visible strings come from the locale files via data attributes: this
    // page does not load the common bundle, and the statuses must not be
    // English-only.
    var MSG = {
        queued: ctxEl ? (ctxEl.dataset.msgQueued || '') : '',
        done: ctxEl ? (ctxEl.dataset.msgDone || '') : '',
        failed: ctxEl ? (ctxEl.dataset.msgFailed || '') : '',
        cancelled: ctxEl ? (ctxEl.dataset.msgCancelled || '') : '',
        count: ctxEl ? (ctxEl.dataset.msgCount || '') : '',
        errorInit: ctxEl ? (ctxEl.dataset.msgErrorInit || '') : '',
        errorSize: ctxEl ? (ctxEl.dataset.msgErrorSize || '') : '',
    };
    var files = [];
    var uploadUrl = null;
    var uploading = false;

    async function getUploadUrl() {
        if (uploadUrl) return uploadUrl;
        try {
            // The password (if any) is carried via the visited_ufs_{token}
            // session cookie set when the upload page was unlocked, never in
            // the URL.
            var resp = await fetch('/api/v2.1/upload-links/' + UPLOAD_TOKEN + '/upload/', { credentials: 'same-origin' });
            if (!resp.ok) throw new Error('Failed to get upload URL');
            var data = await resp.json();
            uploadUrl = data.upload_link;
            return uploadUrl;
        } catch (e) {
            showError(MSG.errorInit.replace('{error}', e.message));
            return null;
        }
    }

    function onFilesSelected(input) {
        if (input.files && input.files.length > 0) {
            for (var i = 0; i < input.files.length; i++) {
                addFile({ file: input.files[i], targetDir: '' });
            }
            input.value = '';
        }
    }

    var dropZone = document.getElementById('drop-zone');
    dropZone.addEventListener('dragover', function (e) { e.preventDefault(); dropZone.dataset.over = 'true'; });
    dropZone.addEventListener('dragleave', function (e) { e.preventDefault(); delete dropZone.dataset.over; });
    dropZone.addEventListener('drop', function (e) {
        e.preventDefault();
        delete dropZone.dataset.over;
        var entries = [];
        for (var i = 0; i < e.dataTransfer.items.length; i++) {
            var item = e.dataTransfer.items[i];
            if (item.kind === 'file') entries.push(item.webkitGetAsEntry());
        }
        collectEntries(entries);
    });

    async function collectEntries(entries) {
        var items = [];
        for (var i = 0; i < entries.length; i++) {
            var collected = await traverseEntry(entries[i], '');
            items = items.concat(collected);
        }
        items.forEach(function (item) { addFile(item); });
    }

    async function traverseEntry(entry, parentPath) {
        var results = [];
        if (!entry) return results;
        if (entry.isFile) {
            var file = await new Promise(function (resolve, reject) { entry.file(resolve, reject); });
            results.push({ file: file, targetDir: parentPath || '', name: entry.name });
        } else if (entry.isDirectory) {
            var reader = entry.createReader();
            var childEntries = await new Promise(function (resolve) { reader.readEntries(function (r) { resolve(r); }); });
            var childPath = parentPath ? parentPath + '/' + entry.name : entry.name;
            for (var j = 0; j < childEntries.length; j++) {
                var children = await traverseEntry(childEntries[j], childPath);
                results = results.concat(children);
            }
        }
        return results;
    }

    function addFile(item) {
        if (MAX_SIZE_MB > 0 && item.file.size > MAX_SIZE_MB * 1024 * 1024) {
            showError(MSG.errorSize.replace('{name}', item.name || item.file.name));
            return;
        }
        files.push({
            id: Date.now() + Math.random(),
            file: item.file,
            name: item.name || item.file.name,
            size: item.file.size,
            state: 'pending',
            progress: 0,
            xhr: null,
        });
        renderFileList();
        startUpload();
    }

    function escapeHtml(str) {
        var div = document.createElement('div');
        div.appendChild(document.createTextNode(str == null ? '' : String(str)));
        return div.innerHTML;
    }

    function escapeAttr(str) {
        return String(str == null ? '' : str)
            .replace(/&/g, '&amp;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;');
    }

    // The extension tile, matching the server-rendered file rows.
    function extOf(name) {
        var dot = String(name).lastIndexOf('.');
        var ext = dot > 0 ? String(name).slice(dot + 1) : '';
        return ext ? ext.slice(0, 4).toUpperCase() : '?';
    }

    function statusOf(f) {
        if (f.state === 'uploading') return { cls: 'text-ink-3', text: Math.round(f.progress || 0) + '%' };
        if (f.state === 'completed') return { cls: 'done text-ok', text: MSG.done };
        if (f.state === 'error') return { cls: 'error text-err', text: MSG.failed };
        if (f.state === 'cancelled') return { cls: 'text-ink-3', text: MSG.cancelled };
        return { cls: 'pending text-ink-3', text: MSG.queued };
    }

    function renderFileList() {
        var container = document.getElementById('file-list');
        var html = '';
        files.forEach(function (f) {
            var st = statusOf(f);
            // The row's own hairline doubles as the progress line while a file
            // is in flight, so progress needs no column of its own.
            var bar = f.state === 'uploading'
                ? '<div class="absolute inset-x-0 bottom-0 h-0.5 bg-raised">' +
                  '<div class="h-full bg-accent transition-[width] duration-200" style="width:' + (f.progress || 0) + '%"></div>' +
                  '</div>'
                : '';
            html += '<div class="file-item relative flex items-center gap-3 px-3.5 py-2 min-h-12 border-b border-line">' +
                '<div class="nf-prow-ic">' + escapeHtml(extOf(f.name)) + '</div>' +
                '<div class="nf-prow-main">' +
                '<div class="nf-prow-name"><span class="base" title="' + escapeAttr(f.name) + '">' + escapeHtml(f.name) + '</span></div>' +
                '</div>' +
                '<div class="shrink-0 whitespace-nowrap text-[12px] tabular-nums text-ink-3">' + formatSize(f.size) + '</div>' +
                '<div class="status ' + st.cls + ' w-16 shrink-0 text-right text-[12px] tabular-nums">' + escapeHtml(st.text) + '</div>' +
                bar +
                '</div>';
        });
        container.innerHTML = html;
        container.classList.toggle('nf-list', files.length > 0);
        updateStatusBar();
    }

    function updateStatusBar() {
        var bar = document.getElementById('status-bar');
        var total = files.length;
        var done = files.filter(function (f) { return f.state === 'completed'; }).length;
        bar.classList.toggle('hidden', total === 0);
        if (total === 0) return;
        document.getElementById('status-count').textContent = MSG.count
            .replace('{done}', String(done))
            .replace('{total}', String(total));
    }

    async function startUpload() {
        if (uploading) return;
        var pending = files.filter(function (f) { return f.state === 'pending'; });
        if (pending.length === 0) return;
        uploading = true;
        var url = await getUploadUrl();
        if (!url) { uploading = false; return; }
        for (var i = 0; i < pending.length; i++) {
            var item = pending[i];
            item.state = 'uploading';
            renderFileList();
            try {
                await uploadFile(item, url);
                item.state = 'completed';
            } catch (e) {
                item.state = 'error';
            }
            renderFileList();
        }
        uploading = false;
        var remaining = files.filter(function (f) { return f.state === 'pending'; });
        if (remaining.length > 0) startUpload();
    }

    function uploadFile(item, url) {
        return new Promise(function (resolve, reject) {
            var formData = new FormData();
            formData.append('file', item.file);
            var xhr = new XMLHttpRequest();
            item.xhr = xhr;
            xhr.upload.onprogress = function (e) {
                if (e.lengthComputable) {
                    item.progress = Math.round((e.loaded / e.total) * 100);
                    renderFileList();
                }
            };
            xhr.onload = function () {
                if (xhr.status >= 200 && xhr.status < 300) resolve();
                else reject(new Error('Upload failed: HTTP ' + xhr.status));
            };
            xhr.onerror = function () { reject(new Error('Network error')); };
            xhr.open('POST', url + '?ret-json=1');
            xhr.send(formData);
        });
    }

    function cancelAll() {
        files.forEach(function (f) {
            if (f.state === 'uploading' || f.state === 'pending') {
                if (f.xhr) f.xhr.abort();
                f.state = 'cancelled';
            }
        });
        uploading = false;
        renderFileList();
    }

    function formatSize(bytes) {
        if (bytes === 0) return '0 B';
        var units = ['B', 'KB', 'MB', 'GB'];
        var i = Math.min(Math.floor(Math.log(bytes) / Math.log(1000)), units.length - 1);
        var val = (bytes / Math.pow(1000, i)).toFixed(i === 0 ? 0 : 1);
        return val + ' ' + units[i];
    }

    function showError(msg) {
        var dropZone = document.getElementById('drop-zone');
        var div = document.createElement('div');
        div.className = 'nf-banner is-err mb-3';
        div.textContent = msg;
        dropZone.parentNode.insertBefore(div, dropZone);
        setTimeout(function () { if (div.parentNode) div.remove(); }, 5000);
    }

    // Delegated handlers (replaces inline onclick/onchange).
    document.addEventListener('click', function (e) {
        var el = e.target.closest('[data-action]');
        if (!el) return;
        if (el.dataset.action === 'pick-files') {
            document.getElementById('file-input').click();
        } else if (el.dataset.action === 'cancel-all') {
            cancelAll();
        }
    });
    document.addEventListener('change', function (e) {
        var el = e.target.closest('[data-action="files-selected"]');
        if (!el) return;
        onFilesSelected(el);
    });
})();
