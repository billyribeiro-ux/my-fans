<script lang="ts">
  import {
    House,
    Compass,
    Bookmark,
    Bell,
    MessageCircle,
    Settings,
    ArrowUpRight,
    Plus,
    ChevronUp,
    Wallet,
    Shield,
    Film,
    Heart,
  } from '@lucide/svelte';
  let {
    active,
    displayName,
    avatarUrl,
    userRole = 'guest',
  }: {
    active: string;
    userRole?: string;
    displayName: string;
    avatarUrl: string;
  } = $props();
  const navigation = [
    { id: 'home', href: '/?tab=following', label: 'Home', icon: House },
    { id: 'discover', href: '/creators', label: 'Discover', icon: Compass },
    { id: 'saved', href: '/?tab=saved', label: 'Saved', icon: Bookmark },
    { id: 'notifications', href: '/notifications', label: 'Notifications', icon: Bell },
    { id: 'messages', href: '/messages', label: 'Messages', icon: MessageCircle },
    { id: 'studio', href: '/studio', label: 'Creator studio', icon: Film },
    { id: 'earnings', href: '/earnings', label: 'Earnings', icon: Wallet },
  ];
</script>

<aside class="sidebar">
  <a class="wordmark" href="/?tab=following" aria-label="MyFans home"
    ><span class="brand-icon"><Heart size={22} fill="currentColor" strokeWidth={0} /></span
    >myfans<span class="wordmark-dot">.</span></a
  >
  <div class="nav-label">YOUR SPACE</div>
  <nav aria-label="Main navigation">
    {#each navigation as item (item.id)}
      <a
        class={[
          'nav-item',
          { active: active === item.id || (item.id === 'home' && active === 'following') },
        ]}
        href={item.href}
        aria-current={active === item.id || (item.id === 'home' && active === 'following')
          ? 'page'
          : undefined}><item.icon size={21} strokeWidth={1.8} /><span>{item.label}</span></a
      >
    {/each}
    {#if userRole === 'admin'}<a
        class={['nav-item', { active: active === 'admin' }]}
        href="/admin"
        aria-current={active === 'admin' ? 'page' : undefined}
        ><Shield size={21} /><span>Administration</span></a
      >{/if}
  </nav>
  <div class="sidebar-bottom">
    <div class="creator-invite">
      <span class="invite-icon"><Plus size={23} /></span>
      <h3>Your passion.<br />Your people.</h3>
      <p>Make a little space<br />for something you love.</p>
      <a href="/studio">Become a creator <ArrowUpRight size={17} /></a>
    </div>
    <a
      class="settings-link"
      href="/settings"
      aria-current={active === 'settings' ? 'page' : undefined}><Settings size={20} /> Settings</a
    >
    <a class="viewer" href={userRole === 'guest' ? '/auth' : '/settings'}
      ><span class="viewer-avatar"
        >{#if avatarUrl}<img
            src={avatarUrl}
            alt=""
            width="42"
            height="42"
          />{:else}{displayName.charAt(0)}{/if}</span
      ><span
        ><strong>{displayName}</strong><small
          >{userRole === 'guest'
            ? 'Sign in to your space'
            : userRole === 'admin'
              ? 'Administrator'
              : 'Your personal space'}</small
        ></span
      ><ChevronUp size={17} /></a
    >
  </div>
</aside>

<nav class="mobile-nav" aria-label="Mobile navigation">
  {#each navigation.filter( (item) => ['home', 'discover', 'messages', 'studio'].includes(item.id) ) as item (item.id)}
    <a
      class={{ active: active === item.id || (item.id === 'home' && active === 'following') }}
      href={item.href}
      aria-label={item.label}
      aria-current={active === item.id || (item.id === 'home' && active === 'following')
        ? 'page'
        : undefined}><item.icon size={22} /><span>{item.label}</span></a
    >
  {/each}
  <a
    href="/settings"
    aria-label="Settings"
    aria-current={active === 'settings' ? 'page' : undefined}
    ><Settings size={22} /><span>Settings</span></a
  >
</nav>

<style>
  .sidebar {
    width: 222px;
    position: sticky;
    top: 0;
    height: 100dvh;
    overflow-y: auto;
    padding: 39px 22px 20px 18px;
    display: flex;
    flex-direction: column;
    border-right: 1px solid var(--border);
  }
  .wordmark {
    display: flex;
    align-items: center;
    font-size: 34px;
    font-weight: 800;
    letter-spacing: -1.8px;
    padding: 0 7px;
    text-align: left;
    width: fit-content;
  }
  .brand-icon {
    color: #e48aa4;
    width: 32px;
    height: 33px;
    margin-right: 7px;
    transform: rotate(-13deg);
  }
  .wordmark-dot {
    color: #e48aa4;
  }
  .nav-label {
    margin: 55px 14px 16px;
    font-size: 10px;
    letter-spacing: 1.9px;
    font-weight: 600;
    color: var(--muted);
  }
  nav {
    display: grid;
    gap: 7px;
  }
  .nav-item {
    height: 48px;
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 0 14px;
    border-radius: 10px;
    color: #6b7365;
    font-size: 13px;
    font-weight: 600;
    text-align: left;
  }
  .nav-item.active {
    background: #e9eee5;
    color: var(--ink);
  }
  .nav-item:hover {
    background: #eff1eb;
  }
  .sidebar-bottom {
    margin-top: auto;
    padding-top: 20px;
  }
  .creator-invite {
    background: #eeeade;
    border-radius: 14px;
    padding: 19px 18px 17px;
  }
  .invite-icon {
    display: flex;
    width: 32px;
    height: 32px;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: #f8f7ee;
    margin-bottom: 15px;
  }
  h3 {
    font-size: 19px;
    line-height: 1.25;
    letter-spacing: -0.6px;
  }
  .creator-invite p {
    color: #636558;
    font-size: 11px;
    line-height: 1.7;
    margin: 9px 0 17px;
  }
  .creator-invite a {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 11px;
    font-weight: 700;
  }
  .settings-link {
    display: flex;
    gap: 14px;
    align-items: center;
    color: var(--muted);
    padding: 24px 14px;
    font-size: 13px;
  }
  .viewer {
    width: 100%;
    border-top: 1px solid var(--border);
    padding-top: 20px;
    display: flex;
    align-items: center;
    text-align: left;
    gap: 10px;
  }
  .viewer-avatar {
    width: 38px;
    height: 38px;
    border-radius: 50%;
    background: #ece2d7;
    display: flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
  }
  .viewer img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .viewer strong {
    display: block;
    font-size: 12px;
    margin-bottom: 5px;
  }
  .viewer small {
    display: block;
    font-size: 9px;
    color: var(--muted);
  }
  .viewer > :global(svg) {
    margin-left: auto;
  }
  .mobile-nav {
    display: none;
  }
  @media (max-height: 900px) and (min-width: 761px) {
    .creator-invite {
      display: none;
    }
    .nav-label {
      margin-top: 32px;
    }
    .sidebar {
      padding-top: 28px;
    }
  }
  @media (max-width: 1190px) {
    .sidebar {
      width: 195px;
      padding-left: 12px;
      padding-right: 15px;
    }
    .wordmark {
      font-size: 30px;
    }
  }
  @media (max-width: 760px) {
    .sidebar {
      display: none;
    }
    .mobile-nav {
      display: flex;
      justify-content: space-around;
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      background: #fafbf7f5;
      backdrop-filter: blur(12px);
      z-index: 20;
      border-top: 1px solid var(--border);
      padding: 9px 8px calc(9px + env(safe-area-inset-bottom));
    }
    .mobile-nav a {
      display: flex;
      flex-direction: column;
      align-items: center;
      gap: 5px;
      padding: 5px 10px;
      color: var(--muted);
      font-size: 9px;
    }
    .mobile-nav a.active {
      color: var(--ink);
    }
  }
</style>
