// Populate the sidebar
//
// This is a script, and not included directly in the page, to control the total size of the book.
// The TOC contains an entry for each page, so if each page includes a copy of the TOC,
// the total size of the page becomes O(n**2).
class MDBookSidebarScrollbox extends HTMLElement {
    constructor() {
        super();
    }
    connectedCallback() {
        this.innerHTML = '<ol class="chapter"><li class="chapter-item expanded "><a href="index.html"><strong aria-hidden="true">1.</strong> Home</a></li><li class="chapter-item expanded affix "><li class="part-title">English</li><li class="chapter-item expanded "><a href="en/overview.html"><strong aria-hidden="true">2.</strong> Architecture Overview</a></li><li class="chapter-item expanded "><a href="en/backend-cli.html"><strong aria-hidden="true">3.</strong> Backend CLI</a></li><li class="chapter-item expanded "><a href="en/setup-wizard.html"><strong aria-hidden="true">4.</strong> Setup Wizard</a></li><li class="chapter-item expanded "><a href="en/zed.html"><strong aria-hidden="true">5.</strong> Zed Integration</a></li><li class="chapter-item expanded "><a href="en/vscode-addon.html"><strong aria-hidden="true">6.</strong> VS Code Addon</a></li><li class="chapter-item expanded "><a href="en/gui.html"><strong aria-hidden="true">7.</strong> GUI Console</a></li><li class="chapter-item expanded affix "><li class="part-title">中文</li><li class="chapter-item expanded "><a href="zh-CN/overview.html"><strong aria-hidden="true">8.</strong> 架构总览</a></li><li class="chapter-item expanded "><a href="zh-CN/backend-cli.html"><strong aria-hidden="true">9.</strong> 后端 CLI</a></li><li class="chapter-item expanded "><a href="zh-CN/setup-wizard.html"><strong aria-hidden="true">10.</strong> 设置向导</a></li><li class="chapter-item expanded "><a href="zh-CN/zed.html"><strong aria-hidden="true">11.</strong> Zed 接入</a></li><li class="chapter-item expanded "><a href="zh-CN/vscode-addon.html"><strong aria-hidden="true">12.</strong> VS Code 插件</a></li><li class="chapter-item expanded "><a href="zh-CN/gui.html"><strong aria-hidden="true">13.</strong> GUI 控制台</a></li></ol>';
        // Set the current, active page, and reveal it if it's hidden
        let current_page = document.location.href.toString().split("#")[0].split("?")[0];
        if (current_page.endsWith("/")) {
            current_page += "index.html";
        }
        var links = Array.prototype.slice.call(this.querySelectorAll("a"));
        var l = links.length;
        for (var i = 0; i < l; ++i) {
            var link = links[i];
            var href = link.getAttribute("href");
            if (href && !href.startsWith("#") && !/^(?:[a-z+]+:)?\/\//.test(href)) {
                link.href = path_to_root + href;
            }
            // The "index" page is supposed to alias the first chapter in the book.
            if (link.href === current_page || (i === 0 && path_to_root === "" && current_page.endsWith("/index.html"))) {
                link.classList.add("active");
                var parent = link.parentElement;
                if (parent && parent.classList.contains("chapter-item")) {
                    parent.classList.add("expanded");
                }
                while (parent) {
                    if (parent.tagName === "LI" && parent.previousElementSibling) {
                        if (parent.previousElementSibling.classList.contains("chapter-item")) {
                            parent.previousElementSibling.classList.add("expanded");
                        }
                    }
                    parent = parent.parentElement;
                }
            }
        }
        // Track and set sidebar scroll position
        this.addEventListener('click', function(e) {
            if (e.target.tagName === 'A') {
                sessionStorage.setItem('sidebar-scroll', this.scrollTop);
            }
        }, { passive: true });
        var sidebarScrollTop = sessionStorage.getItem('sidebar-scroll');
        sessionStorage.removeItem('sidebar-scroll');
        if (sidebarScrollTop) {
            // preserve sidebar scroll position when navigating via links within sidebar
            this.scrollTop = sidebarScrollTop;
        } else {
            // scroll sidebar to current active section when navigating via "next/previous chapter" buttons
            var activeSection = document.querySelector('#sidebar .active');
            if (activeSection) {
                activeSection.scrollIntoView({ block: 'center' });
            }
        }
        // Toggle buttons
        var sidebarAnchorToggles = document.querySelectorAll('#sidebar a.toggle');
        function toggleSection(ev) {
            ev.currentTarget.parentElement.classList.toggle('expanded');
        }
        Array.from(sidebarAnchorToggles).forEach(function (el) {
            el.addEventListener('click', toggleSection);
        });
    }
}
window.customElements.define("mdbook-sidebar-scrollbox", MDBookSidebarScrollbox);
