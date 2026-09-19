// search — infinite-scroll "load more" for search results, plus auto-submit on
// the filename-only filter toggle.
import { formatFileSize } from "./format.js";

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

    // The first server-rendered row doubles as the template for every row the
    // observer appends: the markup is defined once, in templates/search.html,
    // where `data-f` marks the slots this fills.
    var prototype = list.querySelector('li.nf-prow');

    if (!hasMore || !prototype) {
        sentinel.style.display = 'none';
        return;
    }

    function slot(li, name) {
        return li.querySelector('[data-f="' + name + '"]');
    }

    function createResultItem(item) {
        var li = prototype.cloneNode(true);
        slot(li, 'ic-dir').classList.toggle('hidden', !item.is_dir);
        slot(li, 'ic-file').classList.toggle('hidden', !!item.is_dir);

        var link = slot(li, 'name');
        link.href = item.dir_url || '/libraries/' + encodeURIComponent(item.repo_id) + '/files/';
        link.textContent = item.name;

        slot(li, 'repo').textContent = item.repo_name || '';
        slot(li, 'path').textContent = item.fullpath || '';

        var hi = slot(li, 'hi');
        if (item.content_highlight) {
            hi.innerHTML = item.content_highlight;
            hi.classList.remove('hidden');
        } else {
            hi.textContent = '';
            hi.classList.add('hidden');
        }

        slot(li, 'size').textContent = item.is_dir ? '' : formatFileSize(item.size);

        // Left to core/local-time.js, which renders every `[data-ts]` the page
        // gains — so an appended row needs no date formatting of its own. The
        // clone starts with the prototype's rendered text, hence the clear.
        var date = slot(li, 'date');
        date.textContent = '';
        date.title = '';
        date.dataset.ts = String(item.last_modified);
        date.dataset.tsTitle = String(item.last_modified);

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
