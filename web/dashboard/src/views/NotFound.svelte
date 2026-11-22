<script lang="ts">
  import {paths} from '../lib/routes.ts';
  import EmptyState from '../components/EmptyState.svelte';
</script>

<div class="card">
  <EmptyState icon="alert" title="Page not found" text="The dashboard has no page at this address.">
    <a class="btn primary" href={paths.profile}>Go to the profile</a>
  </EmptyState>
</div>
