<script>
  import { onMount, tick } from 'svelte';
  import { invoke } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { relaunch } from '@tauri-apps/plugin-process';
  import { check } from '@tauri-apps/plugin-updater';
  import MarkdownIt from 'markdown-it';
  import hljs from 'highlight.js/lib/core';
  import bash from 'highlight.js/lib/languages/bash';
  import css from 'highlight.js/lib/languages/css';
  import javascript from 'highlight.js/lib/languages/javascript';
  import json from 'highlight.js/lib/languages/json';
  import lua from 'highlight.js/lib/languages/lua';
  import python from 'highlight.js/lib/languages/python';
  import rust from 'highlight.js/lib/languages/rust';
  import typescript from 'highlight.js/lib/languages/typescript';
  import xml from 'highlight.js/lib/languages/xml';
  import {
    Check,
    Copy,
    Download,
    Edit3,
    MessageSquare,
    Plus,
    RotateCcw,
    Save,
    Search,
    Send,
    Settings,
    Square,
    Trash2,
    ChevronRight,
    X,
  } from '@lucide/svelte';

  hljs.registerLanguage('bash', bash);
  hljs.registerLanguage('css', css);
  hljs.registerLanguage('html', xml);
  hljs.registerLanguage('javascript', javascript);
  hljs.registerLanguage('json', json);
  hljs.registerLanguage('lua', lua);
  hljs.registerLanguage('python', python);
  hljs.registerLanguage('rust', rust);
  hljs.registerLanguage('svelte', xml);
  hljs.registerLanguage('typescript', typescript);
  hljs.registerLanguage('xml', xml);

  const markdown = new MarkdownIt({
    html: false,
    linkify: true,
    breaks: true,
    highlight(code, lang) {
      const language = lang && hljs.getLanguage(lang) ? lang : 'plaintext';
      const highlighted = hljs.highlight(code, { language }).value;
      return `<pre class="hljs"><code>${highlighted}</code></pre>`;
    },
  });

  const defaultSettings = {
    backend: 'ollama',
    model: 'gemma4-uncensored-4k:latest',
    ollamaBaseUrl: 'http://127.0.0.1:11434',
    llamaCppBaseUrl: '',
    systemPrompt:
      "follow these style rules:\n- use lowercase only\n- do not use emojis\n- keep responses short and direct\n- avoid long explanations unless i ask for detail\n- if you need to list things, use short bullets\n- talk casually, like a normal person\n- you can swear, use the word nigga and use the word retard and/or retarded to call something fucking stupid",
    reasoningEffort: 'light',
    temperature: 0.7,
    numCtx: 1024,
    numPredict: 1000,
    maxHistoryMessages: 12,
  };

  const effortPresets = [
    { id: 'light', label: 'light', numCtx: 1024, numPredict: 512, maxHistoryMessages: 6 },
    { id: 'medium', label: 'medium', numCtx: 2048, numPredict: 1000, maxHistoryMessages: 12 },
    { id: 'high', label: 'high', numCtx: 4096, numPredict: 1600, maxHistoryMessages: 20 },
    { id: 'extra-high', label: 'extra high', numCtx: 8192, numPredict: 2400, maxHistoryMessages: 32 },
  ];

  let conversations = [];
  let messages = [];
  let activeConversation = null;
  let settings = { ...defaultSettings };
  let draft = '';
  let search = '';
  let models = [];
  let ollamaStatus = 'checking';
  let settingsOpen = false;
  let isLoading = true;
  let isGenerating = false;
  let activeRequestId = '';
  let editingId = null;
  let editingTitle = '';
  let copiedId = null;
  let notice = '';
  let updateCheckStarted = false;
  let pendingUpdate = null;
  let updatePromptOpen = false;
  let updateInstalling = false;
  let sidebarCollapsed = false;
  let modelMenuOpen = false;
  let thoughtOpen = {};
  let messagesEl;
  let composerEl;
  const appWindow = getCurrentWindow();

  onMount(async () => {
    await loadSettings();
    await Promise.all([refreshConversations(), refreshModels()]);
    isLoading = false;
    window.setTimeout(checkForAppUpdate, 900);
  });

  async function loadSettings() {
    try {
      settings = { ...defaultSettings, ...(await invoke('get_settings')) };
    } catch (error) {
      showNotice(`settings load failed: ${formatError(error)}`);
    }
  }

  async function saveSettings() {
    settings = normalizeSettings(settings);
    await invoke('save_settings', { settings });
    settingsOpen = false;
    showNotice('settings saved');
  }

  async function refreshModels() {
    try {
      const localModels = await invoke('list_local_models', {
        backend: 'ollama',
        baseUrl: settings.ollamaBaseUrl,
      });
      models = sortModels(localModels);
      ollamaStatus = models.length > 0 ? 'online' : 'empty';
      if (!models.some((model) => model.name === settings.model) && models[0]) {
        settings = { ...settings, model: models[0].name };
      }
    } catch (error) {
      models = [];
      ollamaStatus = 'offline';
    }
  }

  async function refreshConversations() {
    conversations = await invoke('list_conversations', { search });
  }

  async function createConversation(load = true) {
    const conversation = await invoke('create_conversation', { title: 'new chat' });
    conversations = [conversation, ...conversations];
    if (load) await loadConversation(conversation);
    return conversation;
  }

  async function loadConversation(conversation) {
    if (isGenerating) return;
    activeConversation = conversation;
    messages = await invoke('list_messages', { conversationId: conversation.id });
    await tick();
    scrollToBottom();
    composerEl?.focus();
  }

  function startRename(conversation, event) {
    event.stopPropagation();
    editingId = conversation.id;
    editingTitle = conversation.title;
  }

  async function commitRename(conversation) {
    const title = editingTitle.trim() || 'new chat';
    await invoke('rename_conversation', { conversationId: conversation.id, title });
    editingId = null;
    editingTitle = '';
    if (activeConversation?.id === conversation.id) {
      activeConversation = { ...activeConversation, title };
    }
    await refreshConversations();
  }

  function cancelRename(event) {
    event.stopPropagation();
    editingId = null;
    editingTitle = '';
  }

  async function removeConversation(conversation, event) {
    event.stopPropagation();
    if (!window.confirm(`delete "${conversation.title}"?`)) return;
    await invoke('delete_conversation', { conversationId: conversation.id });
    conversations = conversations.filter((item) => item.id !== conversation.id);
    if (activeConversation?.id === conversation.id) {
      activeConversation = conversations[0] ?? null;
      messages = activeConversation
        ? await invoke('list_messages', { conversationId: activeConversation.id })
        : [];
    }
  }

  async function sendMessage() {
    const content = draft.trim();
    if (!content || isGenerating) return;

    const conversation = activeConversation ?? (await createConversation(false));
    activeConversation = conversation;
    draft = '';

    const savedUserMessage = await invoke('add_message', {
      conversationId: conversation.id,
      role: 'user',
      content,
    });
    messages = [...messages, savedUserMessage];

    if (conversation.title === 'new chat') {
      const title = makeTitle(content);
      await invoke('rename_conversation', { conversationId: conversation.id, title });
      activeConversation = { ...conversation, title };
    }

    await refreshConversations();
    await runGeneration([...messages]);
  }

  async function runGeneration(contextMessages) {
    if (!activeConversation || isGenerating) return;

    isGenerating = true;
    activeRequestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    let assistantContent = '';
    let generationError = '';
    let stopped = false;
    let unlisten = null;

    const pendingMessage = {
      id: `pending-${activeRequestId}`,
      conversation_id: activeConversation.id,
      role: 'assistant',
      content: '',
      created_at: Date.now(),
      pending: true,
    };
    messages = [...contextMessages, pendingMessage];
    await tick();
    scrollToBottom();

    try {
      unlisten = await listen(`ollama-chat-${activeRequestId}`, async (event) => {
        const payload = event.payload;
        if (payload?.content) {
          assistantContent += payload.content;
          messages = messages.map((message) =>
            message.id === pendingMessage.id ? { ...message, content: assistantContent } : message
          );
          await tick();
          scrollToBottom();
        }
        if (payload?.error) generationError = payload.error;
        if (payload?.stopped) stopped = true;
      });

      await invoke('stream_ollama_chat', {
        request: {
          requestId: activeRequestId,
          backend: settings.backend,
          baseUrl: activeBaseUrl(),
          model: settings.model,
          messages: buildOllamaMessages(contextMessages),
          temperature: Number(settings.temperature),
          numCtx: Number(settings.numCtx),
          numPredict: Number(settings.numPredict),
        },
      });
    } catch (error) {
      generationError = generationError || formatError(error);
    } finally {
      unlisten?.();
      const finalText = assistantContent.trim();
      if (finalText) {
        const savedAssistantMessage = await invoke('add_message', {
          conversationId: activeConversation.id,
          role: 'assistant',
          content: stopped ? `${finalText}\n\n[stopped]` : finalText,
        });
        messages = messages.map((message) =>
          message.id === pendingMessage.id ? savedAssistantMessage : message
        );
      } else {
        messages = messages.filter((message) => message.id !== pendingMessage.id);
      }

      if (generationError) showNotice(`generation failed: ${generationError}`);
      isGenerating = false;
      activeRequestId = '';
      await refreshConversations();
      await tick();
      scrollToBottom();
    }
  }

  async function checkForAppUpdate() {
    if (updateCheckStarted) return;
    updateCheckStarted = true;

    try {
      const update = await check({ timeout: 8000 });
      if (!update) return;

      pendingUpdate = update;
      updatePromptOpen = true;
    } catch (error) {
      console.warn('update check failed', error);
    }
  }

  async function installPendingUpdate() {
    if (!pendingUpdate || updateInstalling) return;
    updateInstalling = true;

    try {
      let downloaded = 0;
      showNotice('downloading update');
      await pendingUpdate.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          downloaded = 0;
          showNotice('downloading update');
        }

        if (event.event === 'Progress') {
          downloaded += event.data.chunkLength;
          if (downloaded > 0) showNotice(`downloading update ${formatBytes(downloaded)}`);
        }

        if (event.event === 'Finished') {
          showNotice('installing update');
        }
      });
      await relaunch();
    } catch (error) {
      console.warn('update install failed', error);
      showNotice(`update failed: ${formatError(error)}`);
      updateInstalling = false;
    }
  }

  function dismissUpdatePrompt() {
    if (updateInstalling) return;
    updatePromptOpen = false;
    pendingUpdate = null;
  }

  async function stopGeneration() {
    if (!activeRequestId) return;
    await invoke('stop_ollama_chat', { requestId: activeRequestId });
  }

  async function regenerate(message) {
    if (isGenerating || message.role !== 'assistant') return;
    const index = messages.findIndex((item) => item.id === message.id);
    if (index <= 0) return;
    if (typeof message.id === 'number') {
      await invoke('delete_message', { messageId: message.id });
    }
    const contextMessages = messages.slice(0, index);
    messages = contextMessages;
    await runGeneration(contextMessages);
  }

  async function copyMessage(message) {
    await navigator.clipboard.writeText(message.content);
    copiedId = message.id;
    window.setTimeout(() => {
      if (copiedId === message.id) copiedId = null;
    }, 1200);
  }

  function buildOllamaMessages(sourceMessages) {
    const selected = sourceMessages
      .filter((message) => message.role === 'user' || message.role === 'assistant')
      .slice(-Number(settings.maxHistoryMessages || 12))
      .map((message) => ({ role: message.role, content: message.content }));

    const systemPrompt = settings.systemPrompt?.trim();
    return systemPrompt ? [{ role: 'system', content: systemPrompt }, ...selected] : selected;
  }

  function normalizeSettings(value) {
    const model = !value.model || value.model === 'local-model'
      ? defaultSettings.model
      : value.model;

    return {
      model,
      backend: 'ollama',
      ollamaBaseUrl: value.ollamaBaseUrl || defaultSettings.ollamaBaseUrl,
      llamaCppBaseUrl: '',
      systemPrompt: value.systemPrompt ?? '',
      reasoningEffort: normalizeEffort(value.reasoningEffort),
      temperature: clampNumber(value.temperature, 0, 2, 0.7),
      numCtx: clampNumber(value.numCtx, 512, 8192, 1024),
      numPredict: clampNumber(value.numPredict, 64, 4096, 1000),
      maxHistoryMessages: clampNumber(value.maxHistoryMessages, 2, 40, 12),
    };
  }

  function normalizeEffort(value) {
    if (value === 'custom') return value;
    return effortPresets.some((preset) => preset.id === value) ? value : defaultSettings.reasoningEffort;
  }

  function normalizeBackend(value) {
    return 'ollama';
  }

  function applyEffort(preset) {
    settings = {
      ...settings,
      reasoningEffort: preset.id,
      numCtx: preset.numCtx,
      numPredict: preset.numPredict,
      maxHistoryMessages: preset.maxHistoryMessages,
    };
  }

  function markCustomEffort() {
    settings = { ...settings, reasoningEffort: 'custom' };
  }

  function changeEffort(event) {
    const value = event.currentTarget.value;
    const preset = effortPresets.find((item) => item.id === value);
    if (preset) {
      applyEffort(preset);
    } else {
      markCustomEffort();
    }
  }

  function activeBaseUrl() {
    return settings.ollamaBaseUrl;
  }

  function clampNumber(value, min, max, fallback) {
    const number = Number(value);
    if (!Number.isFinite(number)) return fallback;
    return Math.min(max, Math.max(min, number));
  }

  function makeTitle(content) {
    return content.replace(/\s+/g, ' ').slice(0, 48) || 'new chat';
  }

  function renderMarkdown(content) {
    return markdown.render(content || '');
  }

  function splitThoughts(content) {
    const source = content || '';
    const thoughts = [];
    let answer = source;
    const closedThink = /<think>([\s\S]*?)<\/think>/gi;

    answer = answer.replace(closedThink, (_, thought) => {
      if (thought.trim()) thoughts.push(thought.trim());
      return '';
    });

    const openThink = answer.match(/<think>([\s\S]*)$/i);
    if (openThink) {
      if (openThink[1].trim()) thoughts.push(openThink[1].trim());
      answer = answer.slice(0, openThink.index);
    }

    return {
      thoughts: thoughts.join('\n\n'),
      answer: answer.replace(/<\/?think>/gi, '').trim(),
    };
  }

  function toggleThought(messageId) {
    thoughtOpen = { ...thoughtOpen, [messageId]: !thoughtOpen[messageId] };
  }

  function formatError(error) {
    return typeof error === 'string' ? error : error?.message || 'unknown error';
  }

  function formatBytes(value) {
    if (value < 1024 * 1024) return `${Math.round(value / 1024)} kb`;
    return `${(value / 1024 / 1024).toFixed(1)} mb`;
  }

  function showNotice(message) {
    notice = message;
    window.setTimeout(() => {
      if (notice === message) notice = '';
    }, 2400);
  }

  function scrollToBottom() {
    if (!messagesEl) return;
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function statusText() {
    if (ollamaStatus === 'online') return 'ollama online';
    if (ollamaStatus === 'empty') return 'no models found';
    if (ollamaStatus === 'offline') return 'ollama offline';
    return 'checking ollama';
  }

  function sortModels(value) {
    return [...value].sort((a, b) => modelRank(a.name) - modelRank(b.name) || a.name.localeCompare(b.name));
  }

  function modelRank(name) {
    const ranks = [
      ['gemma4-uncensored-1k', 0],
      ['gemma4-uncensored-2k', 1],
      ['gemma4-uncensored-4k', 2],
      ['gemma4-uncensored-gf', 3],
    ];
    return ranks.find(([prefix]) => name.toLowerCase().startsWith(prefix))?.[1] ?? 99;
  }

  function modelLabel(name) {
    const clean = name.replace(':latest', '');
    const lower = clean.toLowerCase();
    if (lower.includes('gemma4-uncensored-1k')) return 'gemma 4 1k';
    if (lower.includes('gemma4-uncensored-2k')) return 'gemma 4 2k';
    if (lower.includes('gemma4-uncensored-4k')) return 'gemma 4 4k';
    if (lower.includes('gemma4-uncensored-gf')) return 'gemma 4 gf';
    if (lower.includes('gemma-4-uncensored')) return 'gemma 4 original';
    return clean;
  }

  function modelDescription(name) {
    const lower = name.toLowerCase();
    if (lower.includes('-1k')) return 'fastest, smallest context';
    if (lower.includes('-2k')) return 'balanced local context';
    if (lower.includes('-4k')) return 'larger context, heavier';
    if (lower.includes('-gf')) return 'gf variant';
    return 'ollama local model';
  }

  function sendSuggestion(content) {
    draft = content;
    tick().then(() => {
      composerEl?.focus();
      sendMessage();
    });
  }

  function handleComposerKeydown(event) {
    if (event.key !== 'Enter' || event.shiftKey || event.ctrlKey || event.altKey || event.metaKey) return;
    event.preventDefault();
    sendMessage();
  }
</script>
<svelte:window
  on:keydown={(event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === ',') {
      event.preventDefault();
      settingsOpen = true;
    }
  }}
/>

<div class="app" class:sidebar-collapsed={sidebarCollapsed}>
  <aside class="sidebar">
    <div class="sidebar-top" data-tauri-drag-region>
      <div class="traffic-lights">
        <button class="traffic close" title="close" on:click={() => appWindow.close()}></button>
        <button class="traffic minimize" title="minimize" on:click={() => appWindow.minimize()}></button>
        <button class="traffic maximize" title="maximize" on:click={() => appWindow.toggleMaximize()}></button>
      </div>
      <button class="icon-btn" title="toggle sidebar" on:click={() => (sidebarCollapsed = !sidebarCollapsed)}>
        <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="3" y="4" width="18" height="16" rx="3"/><line x1="9.5" y1="4" x2="9.5" y2="20"/></svg>
      </button>
      <div class="brand">gemma local</div>
    </div>

    <button class="new-chat-btn" on:click={() => createConversation()}>
      <Plus size={15} />
      new chat
    </button>

    <label class="search-box">
      <Search size={14} />
      <input bind:value={search} placeholder="search chats" on:input={() => refreshConversations()} />
    </label>

    <div class="conv-list">
      <div class="conv-group-label">conversations</div>
      {#if isLoading}
        <div class="empty-line">loading</div>
      {:else if conversations.length === 0}
        <div class="empty-line">no chats yet</div>
      {/if}

      {#each conversations as conversation (conversation.id)}
        <div
          class="conv-item"
          class:active={activeConversation?.id === conversation.id}
          role="button"
          tabindex="0"
          on:click={() => loadConversation(conversation)}
          on:keydown={(event) => {
            if (event.key === 'Enter' || event.key === ' ') loadConversation(conversation);
          }}
        >
          <span class="dot"></span>
          {#if editingId === conversation.id}
            <input
              class="rename-input"
              bind:value={editingTitle}
              on:click|stopPropagation
              on:keydown={(event) => {
                if (event.key === 'Enter') commitRename(conversation);
                if (event.key === 'Escape') cancelRename(event);
              }}
            />
          {:else}
            <span class="title">{conversation.title}</span>
          {/if}
          <div class="conv-actions">
            <button title="rename" on:click|stopPropagation={(event) => startRename(conversation, event)}><Edit3 size={13} /></button>
            <button title="delete" on:click|stopPropagation={(event) => removeConversation(conversation, event)}><Trash2 size={13} /></button>
          </div>
        </div>
      {/each}
    </div>

  </aside>

  <main class="main">
    <div class="topbar" data-tauri-drag-region>
      <div class="topbar-left">
        <button class="icon-btn expand-btn" title="show sidebar" on:click={() => (sidebarCollapsed = false)}>
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round"><rect x="3" y="4" width="18" height="16" rx="3"/><line x1="9.5" y1="4" x2="9.5" y2="20"/></svg>
        </button>
        <div class="model-picker">
          <button class="model-picker-btn" on:click={() => (modelMenuOpen = !modelMenuOpen)}>
            {modelLabel(settings.model)} <span class="sub">{settings.model.replace(':latest', '')}</span>
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
          </button>
          <div class="model-menu" class:open={modelMenuOpen}>
            {#if models.length === 0}
              <button class="model-option selected" on:click={() => (modelMenuOpen = false)}>
                <span class="swatch" style="background:#86868b"></span>
                <div class="info"><div class="name">{modelLabel(settings.model)}</div><div class="desc">selected local model</div></div>
              </button>
            {/if}
            {#each models as model}
              <button
                class="model-option"
                class:selected={settings.model === model.name}
                on:click={() => {
                  settings = { ...settings, model: model.name };
                  modelMenuOpen = false;
                }}
              >
                <span class="swatch" style="background:#0a84ff"></span>
                <div class="info"><div class="name">{modelLabel(model.name)}</div><div class="desc">{modelDescription(model.name)}</div></div>
                {#if settings.model === model.name}<Check class="check" size={16} />{/if}
              </button>
            {/each}
          </div>
        </div>
      </div>
      <div class="topbar-right">
        <button class="icon-btn" title="refresh models" on:click={refreshModels}><RotateCcw size={17} /></button>
        <button class="icon-btn" title="settings" on:click={() => (settingsOpen = true)}><Settings size={17} /></button>
      </div>
    </div>

    <div class="chat-scroll" bind:this={messagesEl}>
      <div class="chat-inner">
        {#if messages.length === 0}
          <div class="empty-state">
            <h1>what are we building today?</h1>
            <p>local gemma is wired through ollama. keep it short, tweak effort, and chat offline.</p>
            <div class="suggestion-row">
              <button class="suggestion-chip" on:click={() => sendSuggestion('give me a quick powershell command to check ollama models')}>check ollama models</button>
              <button class="suggestion-chip" on:click={() => sendSuggestion('help me optimize my local model settings for 4 gb vram')}>optimize local settings</button>
              <button class="suggestion-chip" on:click={() => sendSuggestion('write a short discord bot command example')}>discord command example</button>
            </div>
          </div>
        {/if}

        {#each messages as message (message.id)}
          {@const parsed = splitThoughts(message.content)}
          <div class="msg-row" class:user={message.role === 'user'} class:assistant={message.role === 'assistant'}>
            <div class="msg-body">
              {#if message.role === 'assistant' && parsed.thoughts}
                <div class="thought-card" class:open={thoughtOpen[message.id]}>
                  <button class="thought-head" on:click={() => toggleThought(message.id)}>
                    <ChevronRight size={14} />
                    thought process
                  </button>
                  {#if thoughtOpen[message.id]}
                    <div class="thought-body">{parsed.thoughts}</div>
                  {/if}
                </div>
              {/if}
              <div class="msg-text" class:pending={message.pending && !message.content}>
                {@html message.content ? renderMarkdown(parsed.answer || (parsed.thoughts ? '' : message.content)) : '<div class="typing-dots"><span></span><span></span><span></span></div>'}
              </div>
              {#if message.role === 'assistant' && !message.pending}
                <div class="msg-actions">
                  <button title="copy" on:click={() => copyMessage(message)}>{#if copiedId === message.id}<Check size={14} />{:else}<Copy size={14} />{/if}</button>
                  <button title="regenerate" on:click={() => regenerate(message)}><RotateCcw size={14} /></button>
                </div>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    </div>

    <form class="composer-wrap" on:submit|preventDefault={sendMessage}>
      <div class="composer">
        <div class="composer-box">
          <textarea bind:this={composerEl} bind:value={draft} rows="1" placeholder="message gemma..." disabled={isGenerating} on:keydown={handleComposerKeydown}></textarea>
          <div class="composer-tools">
            {#if isGenerating}
              <button class="send-btn active" type="button" title="stop" on:click={stopGeneration}><Square size={15} /></button>
            {:else}
              <button class="send-btn" class:active={draft.trim()} type="submit" disabled={!draft.trim()}><Send size={15} /></button>
            {/if}
          </div>
        </div>
        <div class="composer-footer">
          <label class="footer-pill on">
            effort:
            <select bind:value={settings.reasoningEffort} on:change={changeEffort}>
              {#each effortPresets as preset}
                <option value={preset.id}>{preset.label}</option>
              {/each}
              <option value="custom">custom</option>
            </select>
          </label>
          <span>local inference can be wrong. verify important stuff.</span>
        </div>
      </div>
    </form>
  </main>
</div>

<button
  class="overlay"
  class:open={settingsOpen || updatePromptOpen}
  aria-label="close panel"
  on:click={() => {
    settingsOpen = false;
    dismissUpdatePrompt();
  }}
></button>

<div class="panel" class:open={settingsOpen}>
  <div class="panel-head">
    <h2>settings</h2>
    <button class="icon-btn" on:click={() => (settingsOpen = false)}><X size={16} /></button>
  </div>
  <div class="panel-body">
    <div class="settings-section">
      <h3>system prompt</h3>
      <div class="field-desc">custom instructions sent with each request.</div>
      <textarea class="sys-textarea" bind:value={settings.systemPrompt} spellcheck="false"></textarea>
    </div>
    <div class="settings-section">
      <h3>ollama</h3>
      <label class="setting-field"><span>url</span><input bind:value={settings.ollamaBaseUrl} /></label>
      <label class="setting-field"><span>model</span><select bind:value={settings.model}>{#if models.length === 0}<option value={settings.model}>{modelLabel(settings.model)}</option>{/if}{#each models as model}<option value={model.name}>{modelLabel(model.name)}</option>{/each}</select></label>
    </div>
    <div class="settings-section">
      <h3>reasoning</h3>
      <div class="seg-control">
        {#each effortPresets as preset}
          <button class="seg-btn" class:active={settings.reasoningEffort === preset.id} on:click={() => applyEffort(preset)}>{preset.label}</button>
        {/each}
      </div>
    </div>
    <div class="settings-section">
      <h3>model parameters</h3>
      <div class="slider-row"><div class="row-between"><span class="t-label">temperature</span><span class="val">{Number(settings.temperature).toFixed(2)}</span></div><input type="range" min="0" max="2" step="0.05" bind:value={settings.temperature}></div>
      <div class="slider-row"><div class="row-between"><span class="t-label">context</span><span class="val">{settings.numCtx}</span></div><input type="range" min="512" max="8192" step="512" bind:value={settings.numCtx} on:input={markCustomEffort}></div>
      <div class="slider-row"><div class="row-between"><span class="t-label">reply tokens</span><span class="val">{settings.numPredict}</span></div><input type="range" min="64" max="4096" step="64" bind:value={settings.numPredict} on:input={markCustomEffort}></div>
    </div>
  </div>
  <div class="panel-foot">
    <button class="btn btn-ghost" on:click={refreshModels}>refresh models</button>
    <button class="btn btn-primary" on:click={saveSettings}>done</button>
  </div>
</div>

{#if updatePromptOpen && pendingUpdate}
  <div class="panel update-panel open">
    <div class="panel-head">
      <h2>update ready</h2>
      <button class="icon-btn" disabled={updateInstalling} on:click={dismissUpdatePrompt}><X size={16} /></button>
    </div>
    <div class="panel-body">
      <div class="settings-section">
        <h3>new build available</h3>
        <div class="field-desc">version {pendingUpdate.version} is ready. i can install it and restart the app.</div>
      </div>
    </div>
    <div class="panel-foot">
      <button class="btn btn-ghost" disabled={updateInstalling} on:click={dismissUpdatePrompt}>later</button>
      <button class="btn btn-primary" disabled={updateInstalling} on:click={installPendingUpdate}><Download size={15} /> {updateInstalling ? 'installing' : 'install update'}</button>
    </div>
  </div>
{/if}

{#if notice}
  <div class="notice">{notice}</div>
{/if}
