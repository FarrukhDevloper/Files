<script lang="ts">
  // Our own file-type artwork, defined once and referenced by FileIcon via
  // <use>, so a 10k-row listing costs one tiny element per icon. 64×64 grid.
</script>
<svg class="sprite" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">
  <defs>
    <linearGradient id="fi-g-folder" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" style="stop-color: var(--fi-folder-top)" />
      <stop offset="1" style="stop-color: var(--fi-folder-bottom)" />
    </linearGradient>
    <linearGradient id="fi-g-disk" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#f1f2f5" />
      <stop offset="1" stop-color="#b7bbc4" />
    </linearGradient>

    <symbol id="fi-page" viewBox="0 0 64 64">
      <path d="M16 4h22l14 14v38a4 4 0 0 1-4 4H16a4 4 0 0 1-4-4V8a4 4 0 0 1 4-4z"
        style="fill: var(--fi-page); stroke: var(--fi-page-edge)" stroke-width="2" stroke-linejoin="round" />
      <path d="M38 4l14 14H42a4 4 0 0 1-4-4z" style="fill: var(--fi-fold); stroke: var(--fi-page-edge)" stroke-width="2" stroke-linejoin="round" />
    </symbol>

    <symbol id="fi-folder" viewBox="0 0 64 64">
      <path d="M5 14.5a4.5 4.5 0 0 1 4.5-4.5h14a4.5 4.5 0 0 1 3.2 1.3L31 15.5h23.5A4.5 4.5 0 0 1 59 20v30a4.5 4.5 0 0 1-4.5 4.5h-45A4.5 4.5 0 0 1 5 50z"
        style="fill: var(--fi-folder-back)" />
      <path d="M5 23.5a3.5 3.5 0 0 1 3.5-3.5h47a3.5 3.5 0 0 1 3.5 3.5V50a4.5 4.5 0 0 1-4.5 4.5h-45A4.5 4.5 0 0 1 5 50z"
        fill="url(#fi-g-folder)" />
      <path d="M8.5 20.75h47" stroke="#fff" stroke-opacity=".5" stroke-width="1.5" />
      <path d="M22 44.5h20" stroke="#000" stroke-opacity=".08" stroke-width="2" stroke-linecap="round" />
    </symbol>

    <symbol id="fi-doc" viewBox="0 0 64 64">
      <use href="#fi-page" />
    </symbol>

    <symbol id="fi-text" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <g fill="#b9bdc6">
        <rect x="19" y="24" width="26" height="2.4" rx="1.2" />
        <rect x="19" y="30" width="26" height="2.4" rx="1.2" />
        <rect x="19" y="36" width="26" height="2.4" rx="1.2" />
        <rect x="19" y="42" width="26" height="2.4" rx="1.2" />
        <rect x="19" y="48" width="15" height="2.4" rx="1.2" />
      </g>
    </symbol>

    <symbol id="fi-code" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <path d="M26 30l-7 7 7 7M38 30l7 7-7 7M34.5 27l-5 20" fill="none" stroke="#2f7fe0" stroke-width="3.2" stroke-linecap="round" stroke-linejoin="round" />
    </symbol>

    <symbol id="fi-image" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <rect x="18" y="24" width="28" height="22" rx="2.5" fill="#64b8f4" />
      <circle cx="38.5" cy="30.5" r="3.2" fill="#ffd45c" />
      <path d="M18 42l8-9 6 6.5 4-3.5 10 6v2.5a2.5 2.5 0 0 1-2.5 2.5h-23a2.5 2.5 0 0 1-2.5-2.5z" fill="#36a66a" />
    </symbol>

    <symbol id="fi-video" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <rect x="17" y="26" width="30" height="20" rx="4" fill="#8458f0" />
      <g fill="#fff" fill-opacity=".55">
        <rect x="20" y="28" width="3" height="2" rx=".6" /><rect x="20" y="42" width="3" height="2" rx=".6" />
        <rect x="41" y="28" width="3" height="2" rx=".6" /><rect x="41" y="42" width="3" height="2" rx=".6" />
      </g>
      <path d="M29 31.2v9.6a.8.8 0 0 0 1.2.7l7.6-4.8a.8.8 0 0 0 0-1.4l-7.6-4.8a.8.8 0 0 0-1.2.7z" fill="#fff" />
    </symbol>

    <symbol id="fi-audio" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <g fill="#f2385e">
        <circle cx="24.5" cy="44" r="4" />
        <circle cx="37.5" cy="41" r="4" />
        <rect x="26.6" y="28.5" width="2.6" height="15.5" />
        <rect x="39.6" y="25.5" width="2.6" height="15.5" />
        <path d="M26.6 28.5l15.6-3.6v4.6l-15.6 3.6z" />
      </g>
    </symbol>

    <symbol id="fi-archive" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <g fill="#8d9098">
        <rect x="29" y="6" width="3" height="2.2" /><rect x="32" y="9" width="3" height="2.2" />
        <rect x="29" y="12" width="3" height="2.2" /><rect x="32" y="15" width="3" height="2.2" />
        <rect x="29" y="18" width="3" height="2.2" /><rect x="32" y="21" width="3" height="2.2" />
        <rect x="29" y="24" width="3" height="2.2" /><rect x="32" y="27" width="3" height="2.2" />
        <rect x="27.5" y="31" width="9" height="13" rx="2.5" />
      </g>
      <rect x="30" y="37" width="4" height="4.5" rx="1.2" style="fill: var(--fi-page)" />
    </symbol>

    <symbol id="fi-pdf" viewBox="0 0 64 64">
      <use href="#fi-page" />
      <g fill="#c9ccd3">
        <rect x="19" y="14" width="14" height="2.4" rx="1.2" />
        <rect x="19" y="20" width="26" height="2.4" rx="1.2" />
        <rect x="19" y="26" width="26" height="2.4" rx="1.2" />
      </g>
      <rect x="13" y="35" width="38" height="15" fill="#e23b30" />
      <text x="32" y="46.6" text-anchor="middle" fill="#fff" font-size="11" font-weight="800" letter-spacing=".5"
        style="font-family: var(--font-ui)">PDF</text>
    </symbol>

    <symbol id="fi-disk" viewBox="0 0 64 64">
      <rect x="7" y="19" width="50" height="28" rx="7" fill="url(#fi-g-disk)" stroke="#9196a0" stroke-width="1.5" />
      <path d="M10 25h44" stroke="#fff" stroke-opacity=".7" stroke-width="1.5" stroke-linecap="round" />
      <path d="M15 38h22" stroke="#80848e" stroke-width="2.4" stroke-linecap="round" />
      <circle cx="47" cy="38" r="2.6" fill="#33c25b" />
      <path d="M22 52h20" stroke="#9196a0" stroke-opacity=".6" stroke-width="2" stroke-linecap="round" />
    </symbol>

    <symbol id="fi-link" viewBox="0 0 64 64">
      <circle cx="15" cy="51" r="8.5" fill="#fff" stroke="#8d9098" stroke-width="1.5" />
      <path d="M11.5 54.5l7-7M13.5 47.5h5v5" fill="none" stroke="#2c2c2e" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" />
    </symbol>
  </defs>
</svg>

<style>
  .sprite { position: absolute; width: 0; height: 0; overflow: hidden; }
</style>
