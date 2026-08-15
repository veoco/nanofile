// upload — chunked/multi-file upload, drag-and-drop, progress dialog, and
// new-folder creation for the file browser.
import { __t } from "../core/i18n.js";
import { escapeHtml, escapeAttr, getCookie } from "../core/utils.js";
import { formatFileSize, formatBitrate } from "../core/format.js";
import { Toast } from "../core/toast.js";
import { refreshFileList } from "./list.js";
import { registerModalClose } from "../core/modal.js";

// Reads runtime context from the `#upload-context` element (set by toolbar.html):
//   maxUploadSizeMb, currentPath, repoId, repoName.

// ─── Upload state ───
var uploadQueue = [];
var uploadIdCounter = 0;
var isUploadPaused = false;
var isUploadDialogOpen = false;
var dragCounter = 0;
var isDialogMinimized = false;
var uploadTokenUrl = '';
var uploadCtxEl = document.getElementById('upload-context');
var uploadCtx = uploadCtxEl ? uploadCtxEl.dataset : {};
var maxUploadSizeMb = parseInt(uploadCtx.maxUploadSizeMb || '0', 10);

// ─── Chunked upload constants ───
var CHUNK_SIZE = 8 * 1024 * 1024; // 8MB per chunk
var CHUNK_UPLOAD_THRESHOLD = 100 * 1024 * 1024; // 100MB – files above this use chunked upload
var MAX_CHUNK_RETRIES = 3;

// Bitrate tracking
var bitrateLoaded = 0;
var bitrateTimestamp = null;
var currentBitrate = 0;
var bitrateInterval = 500;

// ─── Warn on page leave during active uploads ───
window.addEventListener('beforeunload', function (e) {
    if (uploadQueue.some(function (item) { return item.state === 'uploading' || item.state === 'pending'; })) {
        e.preventDefault();
        e.returnValue = '';
    }
});

// ─── Standard upload (multi-file selection) ───
export function triggerUpload() {
    document.getElementById('file-upload-input').click();
}
export function onFileSelected(input) {
    if (input.files && input.files.length > 0) {
        if (input.hasAttribute('webkitdirectory')) {
            uploadFilesFromFolderInput(input.files);
        } else {
            var items = [];
            for (var i = 0; i < input.files.length; i++) {
                items.push({ file: input.files[i], targetDir: '' });
            }
            addToQueue(items);
        }
        input.value = '';
    }
}

// ─── Folder upload ───
export function triggerFolderUpload() {
    document.getElementById('folder-upload-input').click();
}
export function onFolderSelected(input) {
    if (input.files && input.files.length > 0) {
        uploadFilesFromFolderInput(input.files);
        input.value = '';
    }
}
export function uploadFilesFromFolderInput(files) {
    var items = [];
    for (var i = 0; i < files.length; i++) {
        var file = files[i];
        var parts = file.webkitRelativePath.split('/');
        var targetDir = parts.slice(0, -1).join('/');
        // Extract just the basename from webkitRelativePath — file.name may
        // include the full path in some browsers.
        var fileName = parts[parts.length - 1];
        items.push({ file: file, targetDir: targetDir || '', name: fileName });
    }
    addToQueue(items);
}

// ─── Drag and drop ───
export function onDragOver(e) { e.preventDefault(); }
export function onDragEnter(e) {
    e.preventDefault();
    dragCounter++;
    var overlay = document.querySelector('[data-drop-zone] > .drop-overlay');
    if (overlay && dragCounter === 1) overlay.classList.remove('hidden');
}
export function onDragLeave(e) {
    e.preventDefault();
    dragCounter--;
    if (dragCounter <= 0) {
        dragCounter = 0;
        var overlay = document.querySelector('[data-drop-zone] > .drop-overlay');
        if (overlay) overlay.classList.add('hidden');
    }
}
export function onDrop(e) {
    e.preventDefault();
    dragCounter = 0;
    var overlay = document.querySelector('[data-drop-zone] > .drop-overlay');
    if (overlay) overlay.classList.add('hidden');

    var entries = [];
    for (var i = 0; i < e.dataTransfer.items.length; i++) {
        var item = e.dataTransfer.items[i];
        if (item.kind === 'file') {
            entries.push(item.webkitGetAsEntry());
        }
    }
    if (entries.length > 0) {
        collectAndUploadEntries(entries);
    }
}

export async function collectAndUploadEntries(entries) {
    var items = [];
    for (var i = 0; i < entries.length; i++) {
        var collected = await traverseEntry(entries[i], '');
        items = items.concat(collected);
    }
    if (items.length > 0) addToQueue(items);
}

export async function traverseEntry(entry, parentPath) {
    var results = [];
    if (!entry) return results;
    if (entry.isFile) {
        var file = await new Promise(function (resolve, reject) {
            entry.file(resolve, reject);
        });
        // Use entry.name (always the basename) instead of file.name which
        // may include the full path in some browsers.
        results.push({ file: file, targetDir: parentPath || '', name: entry.name });
    } else if (entry.isDirectory) {
        var reader = entry.createReader();
        var childEntries = await new Promise(function (resolve) {
            reader.readEntries(function (r) { resolve(r); });
        });
        var childPath = parentPath ? parentPath + '/' + entry.name : entry.name;
        for (var j = 0; j < childEntries.length; j++) {
            var children = await traverseEntry(childEntries[j], childPath);
            results = results.concat(children);
        }
    }
    return results;
}

// ─── Queue management ───
export function addToQueue(items) {
    if (items.length === 0) return;

    // Reset bitrate tracking on fresh batch
    bitrateLoaded = 0;
    bitrateTimestamp = null;
    currentBitrate = 0;

    var currentDir = uploadCtx.currentPath;
    var repoId = uploadCtx.repoId;

    items.forEach(function (item) {
        var parentDir;
        if (!item.targetDir) {
            parentDir = currentDir;
        } else if (currentDir === '/') {
            parentDir = '/' + item.targetDir;
        } else {
            parentDir = currentDir + '/' + item.targetDir;
        }

        uploadQueue.push({
            id: ++uploadIdCounter,
            file: item.file,
            name: item.name || item.file.name,
            size: item.file.size,
            parentDir: parentDir,
            state: 'pending',
            progress: 0,
            error: null,
            xhr: null,
        });
    });

    showProgressDialog();
    // Fetch upload token for the upload-aj endpoint
    fetchUploadToken(repoId).then(function () { processQueue(); });
}

// Fetch upload link token via Seafile API
// getCookie is provided by common.js (loaded first).
export async function fetchUploadToken(repoId) {
    try {
        var csrfToken = getCookie('sfcsrftoken');
        var headers = {};
        if (csrfToken) headers['X-CSRFToken'] = csrfToken;

        var resp = await fetch('/api2/repos/' + repoId + '/upload-link/?from=web', {
            credentials: 'same-origin',
            headers: headers,
        });
        if (resp.ok) {
            var data = await resp.json();
            uploadTokenUrl = data;
        }
    } catch (e) {
        console.warn('Failed to fetch upload token:', e);
    }
}

export function processQueue() {
    // Find the first pending item and upload it
    for (var i = 0; i < uploadQueue.length; i++) {
        var item = uploadQueue[i];
        if (item.state === 'pending') {
            if (!isUploadPaused) {
                uploadItem(item);
            }
            return;
        }
    }

    // No more pending items — check if everything is terminal
    var allDone = uploadQueue.every(function (item) {
        return item.state === 'completed' || item.state === 'cancelled';
    });
    if (allDone && uploadQueue.length > 0) {
        refreshFileList();
    }
    updateProgressDialog();
}

export function uploadItem(item) {
    // File size check — reject files exceeding server limit before uploading
    if (maxUploadSizeMb > 0 && item.file.size > maxUploadSizeMb * 1024 * 1024) {
        item.state = 'error';
        item.error = __t('fb.file_too_large', { size: maxUploadSizeMb });
        item.xhr = null;
        updateProgressDialog();
        setTimeout(processQueue, 50);
        return;
    }

    var url = uploadTokenUrl || '/upload-aj/';

    // Large files use chunked upload; small files go direct
    if (item.file.size >= CHUNK_UPLOAD_THRESHOLD) {
        uploadFileInChunks(item, url);
    } else {
        uploadFileDirect(item, url);
    }
}

// ─── Direct (single-request) file upload ───
export function uploadFileDirect(item, url) {
    item.state = 'uploading';
    updateProgressDialog();

    var xhr = new XMLHttpRequest();
    item.xhr = xhr;

    var formData = new FormData();
    formData.append('parent_dir', item.parentDir);
    formData.append('repo_name', uploadCtx.repoName);
    if (!uploadTokenUrl) formData.append('repo_id', uploadCtx.repoId);
    formData.append('file', item.file, item.name);
    formData.append('xhr', '1');

    xhr.upload.onprogress = function (e) {
        if (e.lengthComputable) {
            item.progress = e.total > 0 ? Math.round(e.loaded / e.total * 100) : 0;
            updateBitrate(e.loaded);
            updateProgressDialog();
        }
    };

    xhr.onload = function () {
        if (xhr.status >= 200 && xhr.status < 300) {
            item.state = 'completed';
            item.progress = 100;
        } else {
            var errorMsg = __t('fb.upload_failed_http', { status: xhr.status });
            if (xhr.status === 413) {
                errorMsg = __t('fb.file_too_large_rejected');
            } else {
                try {
                    var resp = JSON.parse(xhr.responseText);
                    if (resp.error_msg) errorMsg = resp.error_msg;
                } catch (_) {}
            }
            item.state = 'error';
            item.error = errorMsg;
        }
        item.xhr = null;
        updateProgressDialog();
        setTimeout(processQueue, 50);
    };

    xhr.onerror = function () {
        item.state = 'error';
        item.error = __t('fb.network_error');
        item.xhr = null;
        updateProgressDialog();
        setTimeout(processQueue, 50);
    };

    xhr.onabort = function () {
        item.xhr = null;
    };

    xhr.open('POST', url);
    xhr.send(formData);
}

// ─── Chunked (resumable) file upload ───

/** Query the server for how many bytes have already been uploaded. */
export function getUploadedBytes(parentDir, fileName) {
    var repoId = uploadCtx.repoId;
    var url = '/api/v2.1/repos/' + repoId + '/file-uploaded-bytes/'
        + '?parent_dir=' + encodeURIComponent(parentDir)
        + '&file_name=' + encodeURIComponent(fileName);
    return fetch(url, { credentials: 'same-origin' }).then(function (r) {
        return r.json();
    }).then(function (data) {
        return data.uploadedBytes || 0;
    });
}

/** Upload a file in 8MB chunks with resume support. */
export function uploadFileInChunks(item, url) {
    item.state = 'uploading';
    updateProgressDialog();
    uploadFileInChunks.tempUrl = url; // hack to avoid passing url through the chain

    var file = item.file;
    var totalChunks = Math.ceil(file.size / CHUNK_SIZE);

    // Check for already-uploaded bytes (resume support)
    getUploadedBytes(item.parentDir, item.name).then(function (uploadedBytes) {
        var resumeChunk = Math.floor(uploadedBytes / CHUNK_SIZE);
        if (resumeChunk > 0) {
            item.progress = Math.round(resumeChunk / totalChunks * 100);
            updateProgressDialog();
        }
        sendNextChunk(item, resumeChunk, totalChunks);
    }).catch(function () {
        // Resume check failed — start from scratch
        sendNextChunk(item, 0, totalChunks);
    });
}

/** Send the next pending chunk.  Recurse until done. */
export function sendNextChunk(item, chunkIndex, totalChunks, retryCount) {
    retryCount = retryCount || 0;
    var file = item.file;
    var start = chunkIndex * CHUNK_SIZE;
    var end = Math.min(file.size, start + CHUNK_SIZE);
    var chunk = file.slice(start, end);
    var isLast = (end >= file.size);

    var formData = new FormData();
    formData.append('parent_dir', item.parentDir);
    formData.append('repo_name', uploadCtx.repoName);
    if (!uploadTokenUrl) formData.append('repo_id', uploadCtx.repoId);
    formData.append('file', chunk, item.name);
    formData.append('xhr', '1');

    var xhr = new XMLHttpRequest();
    item.xhr = xhr;

    xhr.upload.onprogress = function (e) {
        if (e.lengthComputable) {
            // Map this chunk's progress to overall file progress
            var baseProgress = chunkIndex / totalChunks;
            var chunkPart = (e.loaded / e.total) / totalChunks;
            item.progress = Math.round((baseProgress + chunkPart) * 100);
            // Pass cumulative bytes (chunks so far + current progress) for accurate bitrate
            var totalUploaded = chunkIndex * CHUNK_SIZE + e.loaded;
            updateBitrate(totalUploaded);

            // Last chunk: once upload reaches 100%, the server is still
            // processing (CDC + commit).  Show "Processing…" until response.
            if (isLast && e.loaded >= e.total) {
                item.finalizing = true;
                item.progress = 100;
            }

            updateProgressDialog();
        }
    };

    xhr.onload = function () {
        item.xhr = null;
        if (xhr.status >= 200 && xhr.status < 300) {
            if (isLast) {
                // All chunks done
                item.state = 'completed';
                item.progress = 100;
                updateProgressDialog();
                setTimeout(processQueue, 50);
            } else {
                // Send the next chunk
                sendNextChunk(item, chunkIndex + 1, totalChunks);
            }
        } else {
            handleChunkError(item, chunkIndex, totalChunks, retryCount);
        }
    };

    xhr.onerror = function () {
        item.xhr = null;
        handleChunkError(item, chunkIndex, totalChunks, retryCount);
    };

    xhr.onabort = function () {
        item.xhr = null;
    };

    xhr.open('POST', uploadFileInChunks.tempUrl);
    xhr.setRequestHeader('Content-Range', 'bytes ' + start + '-' + (end - 1) + '/' + file.size);
    xhr.send(formData);
}

/** Retry logic for a failed chunk. */
export function handleChunkError(item, chunkIndex, totalChunks, retryCount) {
    if (retryCount < MAX_CHUNK_RETRIES) {
        var delay = 1000 * (retryCount + 1);
        setTimeout(function () {
            sendNextChunk(item, chunkIndex, totalChunks, retryCount + 1);
        }, delay);
    } else {
        item.state = 'error';
        item.error = __t('fb.upload_failed_chunk', { chunk: chunkIndex + 1, total: totalChunks });
        item.xhr = null;
        updateProgressDialog();
        setTimeout(processQueue, 50);
    }
}

// ─── Bitrate calculation ───
export function updateBitrate(loaded) {
    var now = new Date().getTime();
    if (bitrateTimestamp) {
        var timeDiff = now - bitrateTimestamp;
        if (timeDiff >= bitrateInterval) {
            var diff = loaded - bitrateLoaded;
            currentBitrate = diff / (timeDiff / 1000);
            bitrateTimestamp = now;
            bitrateLoaded = loaded;
        }
    } else {
        bitrateTimestamp = now;
        bitrateLoaded = loaded;
    }
}

// ─── Queue actions (per-file) ───

/** Cancel a file — if uploading, abort XHR; mark as cancelled. */
export function cancelUploadItem(id) {
    var item = uploadQueue.find(function (item) { return item.id === id; });
    if (!item || item.state === 'completed' || item.state === 'cancelled') return;
    if (item.state === 'uploading' && item.xhr) {
        item.xhr.abort();
    }
    item.state = 'cancelled';
    item.progress = 0;
    item.error = null;
    item.xhr = null;
    updateProgressDialog();
    processQueue();
}

/** Pause an actively uploading file. */
export function pauseUploadItem(id) {
    var item = uploadQueue.find(function (item) { return item.id === id; });
    if (!item || item.state !== 'uploading') return;
    if (item.xhr) item.xhr.abort();
    item.state = 'paused';
    item.progress = 0;
    item.xhr = null;
    updateProgressDialog();
    processQueue();
}

/** Resume a paused file. */
export function resumeUploadItem(id) {
    var item = uploadQueue.find(function (item) { return item.id === id; });
    if (!item || item.state !== 'paused') return;
    item.state = 'pending';
    item.progress = 0;
    item.error = null;
    updateProgressDialog();
    processQueue();
}

/** Retry a failed or cancelled file. */
export function retryUploadItem(id) {
    var item = uploadQueue.find(function (item) { return item.id === id; });
    if (!item || (item.state !== 'error' && item.state !== 'cancelled')) return;
    item.state = 'pending';
    item.progress = 0;
    item.error = null;
    item.xhr = null;
    updateProgressDialog();
    processQueue();
}

// ─── Bulk queue actions ───

export function pauseAllUploads() {
    isUploadPaused = true;
    uploadQueue.forEach(function (item) {
        if (item.state === 'uploading') {
            if (item.xhr) item.xhr.abort();
            item.state = 'paused';
            item.xhr = null;
        }
    });
    updateProgressDialog();
}

export function resumeAllUploads() {
    isUploadPaused = false;
    // Resume all paused items
    uploadQueue.forEach(function (item) {
        if (item.state === 'paused') {
            item.state = 'pending';
        }
    });
    updateProgressDialog();
    processQueue();
}

export function cancelAllPending() {
    uploadQueue.forEach(function (item) {
        if (item.state === 'uploading') {
            if (item.xhr) item.xhr.abort();
            item.state = 'cancelled';
            item.xhr = null;
        } else if (item.state === 'pending' || item.state === 'paused') {
            item.state = 'cancelled';
        }
    });
    updateProgressDialog();
    processQueue();
}

export function retryAllFailed() {
    uploadQueue.forEach(function (item) {
        if (item.state === 'error' || item.state === 'cancelled') {
            item.state = 'pending';
            item.progress = 0;
            item.error = null;
            item.xhr = null;
        }
    });
    updateProgressDialog();
    processQueue();
}

// ─── Progress Dialog ───

export function showProgressDialog() {
    isUploadDialogOpen = true;
    var dialog = document.getElementById('upload-dialog');
    dialog.classList.remove('hidden');
    updateProgressDialog();
}

export function closeProgressDialog() {
    var hasActive = uploadQueue.some(function (item) {
        return item.state === 'uploading' || item.state === 'pending' || item.state === 'paused';
    });
    if (hasActive) return;

    isUploadDialogOpen = false;
    isDialogMinimized = false;
    var dialog = document.getElementById('upload-dialog');
    dialog.classList.add('hidden');
    dialog.classList.remove('upload-dialog--minimized');
    uploadQueue = [];
    uploadIdCounter = 0;
}

export function toggleMinimize() {
    isDialogMinimized = !isDialogMinimized;
    var dialog = document.getElementById('upload-dialog');
    dialog.classList.toggle('upload-dialog--minimized', isDialogMinimized);
    var btn = document.getElementById('minimize-btn');
    if (btn) btn.textContent = isDialogMinimized ? '□' : '−';
}

export function updateProgressDialog() {
    var dialog = document.getElementById('upload-dialog');
    if (!dialog) return;

    // Auto-close when queue becomes empty
    if (uploadQueue.length === 0) {
        dialog.classList.add('hidden');
        return;
    }

    var total = uploadQueue.length;
    var completed = 0, failed = 0, uploading = 0, pending = 0, paused = 0, cancelled = 0;
    uploadQueue.forEach(function (item) {
        if (item.state === 'completed') completed++;
        else if (item.state === 'error') failed++;
        else if (item.state === 'uploading') uploading++;
        else if (item.state === 'pending') pending++;
        else if (item.state === 'paused') paused++;
        else if (item.state === 'cancelled') cancelled++;
    });

    var isActive = uploading > 0 || pending > 0 || paused > 0;
    var hasFailed = failed > 0;

    // Overall progress (by bytes)
    var totalBytes = 0, uploadedBytes = 0;
    uploadQueue.forEach(function (item) {
        totalBytes += item.size;
        if (item.state === 'completed') uploadedBytes += item.size;
        else if (item.state === 'uploading') uploadedBytes += item.size * item.progress / 100;
    });
    var totalProgress = totalBytes > 0 ? Math.round(uploadedBytes / totalBytes * 100) : 0;

    // ── Header: status + bitrate ──
    var headerEl = dialog.querySelector('.upload-header-status');
    var bitrateEl = dialog.querySelector('.upload-bitrate');

    if (isActive) {
        var statusText = uploading > 0 ? __t('fb.uploading') : (pending > 0 ? __t('fb.waiting') : __t('fb.paused'));
        headerEl.textContent = statusText;
        bitrateEl.textContent = uploading > 0 ? formatBitrate(currentBitrate) : '';
    } else {
        if (hasFailed) {
            headerEl.innerHTML = '<span class="text-red-500">' + __t('fb.failed_upload_count', { count: failed }) + '</span>';
        } else {
            headerEl.textContent = __t('fb.all_uploaded');
        }
        bitrateEl.textContent = '';
    }

    // ── Progress bar (inside header) ──
    var progressBarContainer = dialog.querySelector('.upload-progress-bar-container');
    var progressBar = dialog.querySelector('.upload-progress-bar');
    if (isActive) {
        progressBarContainer.classList.remove('hidden');
        progressBar.style.width = totalProgress + '%';
    } else {
        progressBarContainer.classList.add('hidden');
        progressBar.style.width = '100%';
    }

    // ── Close button visibility ──
    var closeBtn = dialog.querySelector('.js-close-progress');
    if (closeBtn) {
        closeBtn.classList.toggle('hidden', isActive);
    }

    // ── Content bar: count + bulk actions ──
    var countEl = dialog.querySelector('.upload-count');
    countEl.textContent = __t('fb.upload_count', { completed: completed, total: total });

    var actionsEl = dialog.querySelector('.upload-content-actions');
    if (isActive) {
        var pauseBtnHtml = isUploadPaused
            ? '<button class="text-xs text-gray-700 bg-white border border-gray-300 rounded-md px-2 py-1 cursor-pointer whitespace-nowrap hover:bg-gray-50 hover:border-gray-400" data-action="resume-all">' + __t('fb.resume_all') + '</button>'
            : '<button class="text-xs text-gray-700 bg-white border border-gray-300 rounded-md px-2 py-1 cursor-pointer whitespace-nowrap hover:bg-gray-50 hover:border-gray-400" data-action="pause-all">' + __t('fb.pause_all') + '</button>';
        var retryAllBtn = hasFailed
            ? '<button class="text-xs text-gray-700 bg-white border border-gray-300 rounded-md px-2 py-1 cursor-pointer whitespace-nowrap hover:bg-gray-50 hover:border-gray-400" data-action="retry-all">' + __t('fb.retry_all') + '</button>'
            : '';
        var cancelAllBtn = '<button class="text-xs text-red-600 bg-red-50 border border-red-200 rounded-md px-2 py-1 cursor-pointer whitespace-nowrap hover:bg-red-100 hover:border-red-300" data-action="cancel-all">' + __t('fb.cancel_all') + '</button>';
        actionsEl.innerHTML = pauseBtnHtml + retryAllBtn + cancelAllBtn;
    } else {
        var retryAllBtn = hasFailed
            ? '<button class="text-xs text-gray-700 bg-white border border-gray-300 rounded-md px-2 py-1 cursor-pointer whitespace-nowrap hover:bg-gray-50 hover:border-gray-400" data-action="retry-all">' + __t('fb.retry_all') + '</button>'
            : '';
        actionsEl.innerHTML = retryAllBtn;
    }

    // ── File list ──
    var bodyEl = dialog.querySelector('.upload-dialog-body');
    var listHtml = '';
    uploadQueue.forEach(function (item) {
        var statusHtml, actionHtml;

        if (item.state === 'uploading') {
            if (item.finalizing) {
                // All bytes sent, server still processing (CDC + commit)
                statusHtml =
                    '<div class="flex items-center gap-1.5">' +
                      '<svg class="animate-spin h-4 w-4 text-blue-500" fill="none" viewBox="0 0 24 24">' +
                        '<circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4"></circle>' +
                        '<path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"></path>' +
                      '</svg>' +
                      '<span class="text-xs text-blue-600 font-medium">' + __t('fb.processing') + '</span>' +
                    '</div>';
            } else {
                var pct = Math.round(item.progress);
                statusHtml =
                    '<div class="flex items-center gap-2">' +
                      '<span class="inline-block w-28 h-2 bg-gray-200 rounded-full overflow-hidden">' +
                        '<span class="block h-full bg-blue-500 rounded-full" style="width:' + pct + '%;transition:width 0.3s ease"></span>' +
                      '</span>' +
                      '<span class="text-xs text-gray-500 font-medium w-10 text-right">' + pct + '%</span>' +
                    '</div>';
            }
            actionHtml = item.finalizing
                ? ''  // No pause/cancel during server-side processing
                : '<button class="inline-flex items-center justify-center w-8 h-8 rounded-lg text-gray-400 hover:text-gray-600 hover:bg-gray-200 active:scale-90 transition-all" data-action="pause" data-id="' + item.id + '" title="' + __t('fb.pause') + '">' +
                  '<svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="M6 4h4v16H6V4zm8 0h4v16h-4V4z"/></svg>' +
                '</button>' +
                '<button class="inline-flex items-center justify-center w-8 h-8 rounded-lg text-gray-400 hover:text-red-600 hover:bg-red-50 active:scale-90 transition-all" data-action="cancel" data-id="' + item.id + '" title="' + __t('common.cancel') + '">' +
                  '<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
                '</button>';
        } else if (item.state === 'pending') {
            statusHtml = '<span class="text-xs text-gray-400 italic">' + __t('fb.waiting') + '</span>';
            actionHtml =
                '<button class="inline-flex items-center justify-center w-8 h-8 rounded-lg text-gray-400 hover:text-red-600 hover:bg-red-50 active:scale-90 transition-all" data-action="cancel" data-id="' + item.id + '" title="' + __t('common.cancel') + '">' +
                  '<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
                '</button>';
        } else if (item.state === 'paused') {
            statusHtml = '<span class="inline-flex items-center gap-1 text-xs text-amber-600 font-medium"><svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M10 9v6m4-6v6m7-3a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>' + __t('fb.paused') + '</span>';
            actionHtml =
                '<button class="inline-flex items-center justify-center w-8 h-8 rounded-lg text-gray-400 hover:text-blue-600 hover:bg-blue-50 active:scale-90 transition-all" data-action="resume" data-id="' + item.id + '" title="' + __t('fb.resume') + '">' +
                  '<svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>' +
                '</button>' +
                '<button class="inline-flex items-center justify-center w-8 h-8 rounded-lg text-gray-400 hover:text-red-600 hover:bg-red-50 active:scale-90 transition-all" data-action="cancel" data-id="' + item.id + '" title="' + __t('common.cancel') + '">' +
                  '<svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' +
                '</button>';
        } else if (item.state === 'completed') {
            statusHtml = '<span class="inline-flex items-center gap-1 text-xs text-green-600 font-medium"><svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12l2 2 4-4m6 2a9 9 0 11-18 0 9 9 0 0118 0z"/></svg>' + __t('fb.uploaded') + '</span>';
            actionHtml = '';
        } else if (item.state === 'error') {
            statusHtml = '<span class="inline-flex items-center gap-1 text-xs text-red-600 font-medium"><svg class="w-4 h-4 shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 8v4m0 4h.01M21 12a9 9 0 11-18 0 9 9 0 0118 0z"/></svg><span class="truncate max-w-[7rem]">' + escapeHtml(item.error || __t('fb.error')) + '</span></span>';
            actionHtml =
                '<button class="inline-flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded-lg px-3 py-1.5 cursor-pointer hover:bg-blue-100 hover:border-blue-300 active:scale-95 transition-all" data-action="retry" data-id="' + item.id + '">' +
                  '<svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>' +
                  __t('fb.retry') + '</button>';
        } else { // cancelled
            statusHtml = '<span class="inline-flex items-center gap-1 text-xs text-gray-400"><svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M6 18L18 6M6 6l12 12"/></svg>' + __t('fb.cancelled') + '</span>';
            actionHtml =
                '<button class="inline-flex items-center gap-1.5 text-xs font-medium text-blue-700 bg-blue-50 border border-blue-200 rounded-lg px-3 py-1.5 cursor-pointer hover:bg-blue-100 hover:border-blue-300 active:scale-95 transition-all" data-action="retry" data-id="' + item.id + '">' +
                  '<svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>' +
                  __t('fb.retry') + '</button>';
        }

        listHtml +=
            '<div class="flex items-center gap-3 px-3 py-2.5 border-b border-gray-100 hover:bg-gray-50/70 transition-colors">' +
              '<div class="flex-1 min-w-0">' +
                '<div class="flex items-center gap-2">' +
                  '<svg class="w-4 h-4 shrink-0 text-gray-400" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 21h10a2 2 0 002-2V9.414a1 1 0 00-.293-.707l-5.414-5.414A1 1 0 0012.586 3H7a2 2 0 00-2 2v14a2 2 0 002 2z"/></svg>' +
                  '<span class="text-sm text-gray-800 font-medium truncate" title="' + escapeAttr(item.name) + '">' + escapeHtml(item.name) + '</span>' +
                '</div>' +
                '<div class="ml-6 text-xs text-gray-400 mt-0.5">' + formatFileSize(item.size) + '</div>' +
              '</div>' +
              '<div class="shrink-0">' + statusHtml + '</div>' +
              '<div class="shrink-0 flex items-center gap-0.5">' + actionHtml + '</div>' +
            '</div>';
    });
    bodyEl.innerHTML = listHtml;
}

// ─── New Folder Dialog ───
export function showNewFolderDialog() {
    document.getElementById('new-folder-overlay').classList.remove('hidden');
    document.getElementById('new-folder-input').focus();
}
export function hideNewFolderDialog() {
    document.getElementById('new-folder-overlay').classList.add('hidden');
    document.getElementById('new-folder-input').value = '';
}
export function submitNewFolder(form) {
    var name = document.getElementById('new-folder-input').value.trim();
    if (!name) return false;
    var currentDir = form.querySelector('[name="current_dir"]').value;
    var p = currentDir === '/' ? '/' + name : currentDir + '/' + name;
    hideNewFolderDialog();

    // Use fetch to call Seafile API
    var repoId = uploadCtx.repoId;
    var csrfToken = form.querySelector('[name="csrf_token"]').value || '';
    fetch('/api/v2.1/repos/' + repoId + '/dir/', {
        method: 'POST',
        credentials: 'same-origin',
        headers: {
            'Content-Type': 'application/json',
            'X-CSRFToken': csrfToken,
        },
        body: JSON.stringify({ p: p }),
    }).then(function (resp) {
        if (resp.ok) {
            Toast.success(__t('fb.folder_created', { name: name }));
            refreshFileList();
        } else {
            resp.text().then(function (t) { Toast.error(__t('fb.failed_prefix') + t); });
        }
    }).catch(function () {
        Toast.error(__t('fb.network_error_folder'));
    });

    return false;
}

// ─── Event delegation (replaces inline onclick/onchange/ondrag handlers) ──

document.addEventListener("click", function (e) {
    var btn = e.target.closest("[data-action]");
    if (!btn) return;
    var action = btn.dataset.action;
    var id = btn.dataset.id;

    switch (action) {
        case "new-folder": showNewFolderDialog(); break;
        case "upload": triggerUpload(); break;
        case "upload-folder": triggerFolderUpload(); break;
        case "toggle-minimize": toggleMinimize(); break;
        case "close-progress": closeProgressDialog(); break;
        case "close-new-folder": hideNewFolderDialog(); break;
        case "pause": pauseUploadItem(Number(id)); break;
        case "cancel": cancelUploadItem(Number(id)); break;
        case "resume": resumeUploadItem(Number(id)); break;
        case "retry": retryUploadItem(Number(id)); break;
        case "pause-all": pauseAllUploads(); break;
        case "resume-all": resumeAllUploads(); break;
        case "cancel-all": cancelAllPending(); break;
        case "retry-all": retryAllFailed(); break;
    }
});

document.addEventListener("change", function (e) {
    var el = e.target.closest("[data-action]");
    if (!el) return;
    if (el.dataset.action === "file-select") onFileSelected(el);
    else if (el.dataset.action === "folder-select") onFolderSelected(el);
});

document.addEventListener("submit", function (e) {
    var form = e.target.closest('[data-form="new-folder"]');
    if (!form) return;
    e.preventDefault();
    submitNewFolder(form);
});

document.addEventListener("keydown", function (e) {
    if (!e.target.closest("#new-folder-input")) return;
    if (e.key === "Escape") {
        e.preventDefault();
        hideNewFolderDialog();
    } else if (e.key === "Enter") {
        e.preventDefault();
        var submitBtn = e.target.form ? e.target.form.querySelector('button[type="submit"]') : null;
        if (submitBtn) submitBtn.click();
    }
});

// Drag-and-drop — delegated so it keeps working after the file list is
// partially refreshed (the [data-drop-zone] container is re-rendered).
["dragover", "dragenter", "dragleave", "drop"].forEach(function (type) {
    document.addEventListener(type, function (e) {
        if (!e.target.closest("[data-drop-zone]")) return;
        if (type === "dragover") onDragOver(e);
        else if (type === "dragenter") onDragEnter(e);
        else if (type === "dragleave") onDragLeave(e);
        else onDrop(e);
    });
});

registerModalClose("hideNewFolderDialog", hideNewFolderDialog);

