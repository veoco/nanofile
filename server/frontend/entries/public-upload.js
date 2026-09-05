// public-upload — the public upload-link page (standalone layout, no common
// bundle). Migrated from the inline <script> block in
// templates/web/upload_link_view.html; driven by data-* event delegation.
(function () {
    var ctxEl = document.getElementById('public-upload-context');
    var UPLOAD_TOKEN = ctxEl ? (ctxEl.dataset.uploadToken || '') : '';
    var MAX_SIZE_MB = ctxEl ? parseInt(ctxEl.dataset.maxSizeMb || '0', 10) : 0;
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
            showError('Failed to initialize upload: ' + e.message);
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
    dropZone.addEventListener('dragover', function (e) { e.preventDefault(); dropZone.classList.add('dragover'); });
    dropZone.addEventListener('dragleave', function (e) { e.preventDefault(); dropZone.classList.remove('dragover'); });
    dropZone.addEventListener('drop', function (e) {
        e.preventDefault();
        dropZone.classList.remove('dragover');
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
            showError('File "' + (item.name || item.file.name) + '" exceeds maximum upload size');
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

    function renderFileList() {
        var container = document.getElementById('file-list');
        var html = '';
        files.forEach(function (f) {
            var sizeStr = formatSize(f.size);
            var statusHtml = '';
            if (f.state === 'uploading') {
                var pct = f.progress || 0;
                statusHtml = '<div class="progress-bar"><div class="fill" style="width:' + pct + '%"></div></div>';
            } else if (f.state === 'completed') {
                statusHtml = '<span class="status done">Done</span>';
            } else if (f.state === 'error') {
                statusHtml = '<span class="status error">Failed</span>';
            } else {
                statusHtml = '<span class="status">Queued</span>';
            }
            html += '<div class="file-item">' +
                '<span class="name" title="' + escapeAttr(f.name) + '">' + escapeHtml(f.name) + '</span>' +
                '<span class="size">' + sizeStr + '</span>' +
                '<div class="status">' + statusHtml + '</div>' +
                '</div>';
        });
        container.innerHTML = html;
        updateStatusBar();
    }

    function updateStatusBar() {
        var bar = document.getElementById('status-bar');
        var total = files.length;
        var done = files.filter(function (f) { return f.state === 'completed'; }).length;
        if (total === 0) { bar.style.display = 'none'; return; }
        bar.style.display = 'flex';
        document.getElementById('status-count').textContent = done + ' / ' + total + ' files';
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
        var container = document.querySelector('.card');
        var div = document.createElement('div');
        div.className = 'warning-banner';
        div.textContent = msg;
        container.insertBefore(div, container.querySelector('.drop-zone'));
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
