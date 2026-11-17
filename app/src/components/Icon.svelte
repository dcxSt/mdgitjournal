<script lang="ts" module>
  // 16 × 16 stroke icons. Zero-length segments with round caps draw dots.
  const ICONS = {
    pull: ["M8 2.5v10", "M4 8.5l4 4 4-4"],
    push: ["M8 13.5v-10", "M4 7.5l4-4 4 4"],
    plus: ["M8 3v10", "M3 8h10"],
    settings: [
      "M2 5h3",
      "M9 5h5",
      "M5 5a2 2 0 1 0 4 0a2 2 0 1 0-4 0",
      "M2 11h5",
      "M11 11h3",
      "M7 11a2 2 0 1 0 4 0a2 2 0 1 0-4 0",
    ],
    more: ["M3.5 8h.01", "M8 8h.01", "M12.5 8h.01"],
    back: ["M10 3L5 8l5 5"],
    close: ["M4 4l8 8", "M12 4l-8 8"],
    search: ["M2.5 7a4.5 4.5 0 1 0 9 0a4.5 4.5 0 1 0-9 0", "M10.5 10.5L14 14"],
    link: ["M7 9a2.8 2.8 0 0 0 4 0l2-2a2.8 2.8 0 0 0-4-4l-.7.7", "M9 7a2.8 2.8 0 0 0-4 0l-2 2a2.8 2.8 0 0 0 4 4l.7-.7"],
    camera: ["M2 5.5h2.8l1-1.5h4.4l1 1.5H14v7H2z", "M6 8.8a2 2 0 1 0 4 0a2 2 0 1 0-4 0"],
  } as const;

  export type IconName = keyof typeof ICONS;
</script>

<script lang="ts">
  let { name, size = 14 }: { name: IconName; size?: number } = $props();
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width={name === "more" ? 2 : 1.4}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
>
  {#each ICONS[name] as d (d)}
    <path {d} />
  {/each}
</svg>

<style>
  svg {
    flex: none;
    display: block;
  }
</style>
