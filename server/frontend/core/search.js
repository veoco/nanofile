// search — infinite-scroll "load more" for search results, plus auto-submit on
// the filename-only filter toggle. Migrated from the inline <script> block in
// templates/search.html.
(function () {
    var list = document.getElementById('search-results');
    if (!list) return;

    var sentinel = document.getElementById('scroll-sentinel');
    var spinner = document.getElementById('loading-spinner');
    if (!sentinel) return;

    var page = parseInt(list.dataset.page) || 1;
    var perPage = parseInt(list.dataset.perPage) || 20;
    var query = list.dataset.query;
    var filenameOnly = list.dataset.searchFilenameOnly !== 'false'; // default true
    var hasMore = list.dataset.hasMore === 'true';
    var loading = false;

    if (!hasMore) {
        sentinel.style.display = 'none';
        return;
    }

    function formatSize(bytes) {
        if (bytes === 0) return '0 B';
        var units = ['B', 'KB', 'MB', 'GB', 'TB'];
        var i = Math.floor(Math.log(bytes) / Math.log(1000));
        return (bytes / Math.pow(1000, i)).toFixed(i > 0 ? 1 : 0) + ' ' + units[i];
    }

    function formatDate(ts) {
        var d = new Date(ts * 1000);
        var y = d.getFullYear();
        var m = ('0' + (d.getMonth() + 1)).slice(-2);
        var day = ('0' + d.getDate()).slice(-2);
        var h = ('0' + d.getHours()).slice(-2);
        var min = ('0' + d.getMinutes()).slice(-2);
        return y + '-' + m + '-' + day + ' ' + h + ':' + min;
    }

    function createResultItem(item) {
        var li = document.createElement('li');
        li.className = 'px-4 py-3 flex items-center gap-x-3 hover:bg-gray-50 dark:hover:bg-surface-700/50 transition-colors';

        // Icon
        var iconSpan = document.createElement('span');
        iconSpan.className = 'flex-shrink-0 text-gray-400 dark:text-gray-500';
        if (item.is_dir) {
            iconSpan.innerHTML = '<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M3 7v10a2 2 0 002 2h14a2 2 0 002-2V9a2 2 0 00-2-2h-6l-2-2H5a2 2 0 00-2 2z"/></svg>';
        } else {
            iconSpan.innerHTML = '<svg class="h-5 w-5" fill="none" viewBox="0 0 24 24" stroke="currentColor"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"/></svg>';
        }
        li.appendChild(iconSpan);

        // File info
        var infoDiv = document.createElement('div');
        infoDiv.className = 'min-w-0 flex-1';

        var link = document.createElement('a');
        link.href = item.dir_url || '/libraries/' + encodeURIComponent(item.repo_id) + '/files/';
        link.className = 'text-sm font-medium text-brand-500 dark:text-brand-400 hover:text-brand-600 dark:hover:text-brand-300 truncate block';
        link.textContent = item.name;
        infoDiv.appendChild(link);

        var metaP = document.createElement('p');
        metaP.className = 'text-xs text-green-600 dark:text-green-400 truncate';
        metaP.textContent = item.repo_name + ' \u00b7 ' + item.fullpath;
        infoDiv.appendChild(metaP);

        if (item.content_highlight) {
            var snippetP = document.createElement('p');
            snippetP.className = 'mt-1 text-xs text-gray-500 dark:text-gray-400 leading-relaxed';
            snippetP.innerHTML = item.content_highlight;
            infoDiv.appendChild(snippetP);
        }

        li.appendChild(infoDiv);

        // Size and date
        var metaDiv = document.createElement('div');
        metaDiv.className = 'flex-shrink-0 text-right text-xs text-gray-400 dark:text-gray-500';
        if (!item.is_dir) {
            var sizeDiv = document.createElement('div');
            sizeDiv.textContent = formatSize(item.size);
            metaDiv.appendChild(sizeDiv);
        }
        var dateDiv = document.createElement('div');
        dateDiv.textContent = formatDate(item.last_modified);
        metaDiv.appendChild(dateDiv);
        li.appendChild(metaDiv);

        return li;
    }

    var observer = new IntersectionObserver(async function (entries) {
        for (var i = 0; i < entries.length; i++) {
            var entry = entries[i];
            if (entry.isIntersecting && hasMore && !loading) {
                loading = true;
                spinner.classList.remove('hidden');

                try {
                    page++;
                    var params = new URLSearchParams({ q: query, page: page, per_page: perPage });
                    params.set('search_filename_only', filenameOnly ? 'true' : 'false');

                    var resp = await fetch('/api2/search/?' + params.toString());
                    var data = await resp.json();

                    for (var j = 0; j < data.results.length; j++) {
                        list.appendChild(createResultItem(data.results[j]));
                    }

                    hasMore = data.has_more;
                    if (!hasMore) {
                        sentinel.style.display = 'none';
                        observer.unobserve(sentinel);
                    }
                } catch (e) {
                    console.error('Failed to load more results:', e);
                } finally {
                    loading = false;
                    spinner.classList.add('hidden');
                }
            }
        }
    }, { rootMargin: '200px' });

    observer.observe(sentinel);
})();

// Auto-submit the search form when the filename-only filter is toggled.
document.addEventListener("change", function (e) {
    var el = e.target.closest("[data-auto-submit]");
    if (!el || !el.form) return;
    el.form.submit();
});
