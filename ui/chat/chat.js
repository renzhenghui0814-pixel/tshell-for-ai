(function () {
  'use strict';

  const vscode = acquireVsCodeApi();
  const boot = window.__tshellChat || {};
  const S = boot.strings || {};
  const SVG_NS = 'http://www.w3.org/2000/svg';

  const $ = (id) => document.getElementById(id);
  const post = (type, payload) => vscode.postMessage(Object.assign({ type: type }, payload || {}));

  const thread = $('thread');
  /** The card awaiting a result, so output lands on the command that produced it. */
  let openCard = null;
  /** The progress view inside it, while a transfer is running. Ticks repaint this. */
  let openTransfer = null;
  /** Labels for the copy button on a rendered code block. */
  const copyStrings = { copy: S.agentCopy, copied: S.agentCopied };
  /*
   * Whether the model's own thinking is drawn. Live rather than read once: the
   * thinking window switches it, and so does another panel's. It takes effect on
   * the steps that follow -- cards already in the thread are left as they were
   * drawn, because the thinking behind the ones that were hidden was never kept
   * and could not be put back even if the switch asked for it.
   *
   * Kept beside `thinking.show` rather than read out of it on every fragment:
   * this is touched once per streamed chunk, and the settings object is not.
   */
  let showReasoning = (boot.thinking && boot.thinking.show) === true;

  function icon(id, className) {
    const svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('class', className || 'icon');
    svg.setAttribute('aria-hidden', 'true');
    const use = document.createElementNS(SVG_NS, 'use');
    use.setAttribute('href', id);
    svg.append(use);
    return svg;
  }

  function applyStrings() {
    document.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = S[el.dataset.i18n] || ''; });
    document.querySelectorAll('[data-i18n-placeholder]').forEach((el) => { el.placeholder = S[el.dataset.i18nPlaceholder] || ''; });
    // An icon button says what it does through its tooltip and its label.
    document.querySelectorAll('[data-i18n-title]').forEach((el) => {
      const label = S[el.dataset.i18nTitle] || '';
      el.title = label;
      el.setAttribute('aria-label', label);
    });
  }

  /**
   * Moves the keyboard within this panel, and never into it.
   *
   * A confirmation arrives when the assistant reaches a step it cannot take on
   * its own, which is exactly when the user is somewhere else -- reading the
   * terminal, typing in another editor, working while it works. Focusing the
   * dialog outright pulled them out of whatever they were doing mid-keystroke,
   * and the keystroke went to the dialog.
   *
   * So focus is only ever taken when this panel already had it. The prompt is
   * still revealed and still answerable: the keydown handler is on the document
   * with capture, so one click anywhere in the panel and the arrows, the number
   * keys, Enter and Esc all work as before.
   */
  function takeFocus(el) {
    if (el && document.hasFocus()) el.focus();
  }

  function atBottom() {
    return thread.scrollTop + thread.clientHeight >= thread.scrollHeight - 40;
  }

  /*
   * Applies a change to the thread and keeps the end in view if it was already
   * showing. Whether to follow is decided before the change, because once a card
   * has grown the view is no longer at the bottom, so deciding afterwards would
   * always read as "the user scrolled away" and the thread would stop following.
   *
   * The new offset is written in the same task as the change that caused it, so
   * the browser resolves both in one layout and paints them together. Deferring
   * it to a later frame lets the scroller move under content it has already
   * painted, which leaves a band of the previous frame behind.
   */
  function follow(change) {
    const stick = atBottom();
    const result = change();
    if (stick) thread.scrollTop = thread.scrollHeight;
    return result;
  }

  function append(node) {
    return follow(() => {
      $('empty').hidden = true;
      thread.append(node);
      return node;
    });
  }

  /**
   * Puts one step on the timeline.
   *
   * The dot and the rail are drawn by a wrapper and never by the thing itself.
   * Hanging them off the card was the obvious way and the wrong one: a card sets
   * `overflow: hidden` so its rounded corner clips the code block inside it, and
   * that clips its own pseudo-elements too -- the dot landed inside the card's
   * background and the line to the next step was cut off at the border. A wrapper
   * has no background, no radius and nothing to clip, so the rail is drawn beside
   * the content instead of on top of it.
   *
   * @returns the node passed in, not the wrapper: callers hold on to cards.
   */
  function appendStep(node, tone) {
    const row = document.createElement('div');
    row.className = 'step' + (tone ? ' ' + tone : '');
    row.append(node);
    append(row);
    return node;
  }

  /** The timeline row something sits in, for removing it whole. */
  function stepOf(node) {
    return node && node.closest ? node.closest('.step') : null;
  }

  /*
   * Following the end of the thread during a stream, at most once a frame.
   *
   * `follow` reads the scroll geometry and writes it back around every change,
   * which is right for one insertion and wrong thirty times a second. Each call
   * forces a synchronous layout, and a scroll position written from script in the
   * middle of a burst of mutations is exactly what leaves this panel painting
   * stale tiles -- rows drawn over rows, frozen until a hover or a resize forces
   * a repaint. That failure is described at length in the stylesheet; this is the
   * other way into it.
   *
   * So a streaming repaint does not scroll. It asks to be followed, the asks
   * coalesce, and one scroll happens in the next frame once the DOM has settled.
   */
  let followFrame = 0;
  let followWanted = false;

  /**
   * The streaming counterpart of `follow`: the same sample, a deferred write.
   *
   * The sample is taken before the change for the same reason `follow` takes it
   * there, and it is the whole of what this function exists to get right. Read
   * afterwards, the question becomes "is the view at the bottom of a thread that
   * has already grown", and one repaint adding more than the tolerance answers no
   * -- which is indistinguishable from the user having scrolled away, so following
   * stops. It does not resume either: the view only falls further behind from
   * there, so every later sample says no as well. A reply gaining a table row or
   * two lines of a code block clears the tolerance easily, which is why a long
   * answer used to run off the bottom of the panel and stay there.
   */
  function followLater(change) {
    // Sampled once per burst: after the scroll the answer is always "yes", so
    // re-asking mid-burst would never let go of a thread the user has left.
    if (!followWanted) followWanted = atBottom();
    const result = change();
    if (followFrame) return result;
    followFrame = requestAnimationFrame(() => {
      followFrame = 0;
      const bottom = thread.scrollHeight - thread.clientHeight;
      // Skipped when it would not move anything. A stream sitting at the bottom
      // produces a run of writes that change nothing and dirty the scroller all
      // the same, which is the cheapest half of this problem to stop causing.
      if (followWanted && Math.abs(thread.scrollTop - bottom) > 1) thread.scrollTop = bottom;
      followWanted = false;
    });
    return result;
  }

  function note(text, kind) {
    const el = document.createElement('div');
    el.className = 'note' + (kind ? ' ' + kind : '');
    el.append(document.createTextNode(text));
    return appendStep(el);
  }

  /*
   * The assistant writes markdown whether or not it is asked to, so its turns go
   * through the renderer. The user's own turn does not: what they typed is shown
   * back exactly as they typed it, backticks and all.
   */
  function turn(text, who) {
    const el = document.createElement('div');
    el.className = 'turn ' + who;
    const bubble = document.createElement('div');
    bubble.className = 'bubble';
    if (who === 'assistant') bubble.append(window.tshellMarkdown.render(text, copyStrings));
    else bubble.textContent = text;
    el.append(bubble);
    /*
     * The user's own turns are on the rail too, in blue. The timeline is the
     * shape of the whole conversation and not of the machine's half of it: it
     * starts at the message that set a task going, and the steps hanging below
     * are what that message caused. Breaking the line at every question would
     * cut it exactly where the reason for the next dozen steps is written.
     */
    appendStep(el, who === 'user' ? 'sent' : '');
  }

  // -- streaming ------------------------------------------------------------

  /** The bubble an answer is being written into, while it is still arriving. */
  let liveBubble = null;
  /** The two halves inside it: what is finished, and what is still growing. */
  let liveDone = null;
  let liveTail = null;
  let liveText = '';
  /** How much of `liveText` has been rendered into `liveDone` and never will be again. */
  let liveSettled = 0;
  let liveTimer = null;

  /**
   * How much of the text can never be parsed differently by anything that
   * follows it.
   *
   * A blank line ends a block -- unless a fence is open across it, in which case
   * it is not a blank line at all but a blank line of code. Counting the fences
   * in the candidate prefix is what tells those two apart.
   */
  function stableCut(text) {
    for (let cut = text.lastIndexOf('\n\n'); cut > 0; cut = text.lastIndexOf('\n\n', cut - 1)) {
      const head = text.slice(0, cut + 1);
      if ((head.match(/^[ \t]*(?:```|~~~)/gm) || []).length % 2 === 0) return cut + 1;
    }
    return 0;
  }

  /*
   * Markdown is rendered while the text is still arriving, not after it.
   *
   * The obvious objection is the half-written fence: for a moment `​```sh` has
   * opened and not closed, and the lines under it are prose. The renderer already
   * answers that -- an unclosed fence runs to the end of the text -- so what the
   * reader sees is a code block that is still being filled in, which is what is
   * actually happening. Every other shape settles the same way: a heading is a
   * heading from its first `#`, a list item from its bullet.
   *
   * What it must not cost is the whole reply, re-rendered eight times a second.
   * A finished block is finished -- a table above the cursor will parse the same
   * way whatever arrives next -- so it is rendered once and left alone, and only
   * the growing tail is thrown away and rebuilt. Rebuilding everything is how a
   * long answer containing a table recreated a nested scroller on every tick,
   * which is the same churn that had rows painting over rows.
   */
  function livePace() {
    return liveText.length > 24000 ? 300 : liveText.length > 8000 ? 200 : 120;
  }

  function paintLive() {
    liveTimer = null;
    if (!liveBubble) return;
    followLater(() => {
      const cut = stableCut(liveText);
      if (cut > liveSettled) {
        // Appended, never re-rendered: these blocks are settled by definition.
        liveDone.append(window.tshellMarkdown.render(liveText.slice(liveSettled, cut), copyStrings));
        liveSettled = cut;
      }
      liveTail.replaceChildren(window.tshellMarkdown.render(liveText.slice(liveSettled), copyStrings));
    });
  }

  function appendDelta(text) {
    if (!liveBubble) {
      clearThinking();
      const el = document.createElement('div');
      el.className = 'turn assistant';
      liveBubble = document.createElement('div');
      liveBubble.className = 'bubble streaming';
      // Both are `display: contents`, so the blocks inside them lay out as though
      // the split were not there. It exists for the renderer, not for the reader.
      liveDone = document.createElement('div');
      liveDone.className = 'md-done';
      liveTail = document.createElement('div');
      liveTail.className = 'md-live';
      liveBubble.append(liveDone, liveTail);
      liveText = '';
      liveSettled = 0;
      el.append(liveBubble);
      appendStep(el);
    }
    liveText += text;
    countOutput(text);
    if (!liveTimer) liveTimer = setTimeout(paintLive, livePace());
  }

  function stopLive() {
    if (liveTimer) { clearTimeout(liveTimer); liveTimer = null; }
  }

  /**
   * Settles the live bubble on the text that actually arrived.
   *
   * @param text What the model really sent. The streamed text is only what was
   *   painted early, and the two can differ: a retry throws away a half-written
   *   answer, and an endpoint that ignored `stream` never painted anything.
   * @returns true when a bubble was there to settle, so the caller knows not to
   *   add a second one.
   */
  function settleDelta(text) {
    if (!liveBubble) return false;
    stopLive();
    const bubble = liveBubble;
    liveBubble = null;
    liveDone = null;
    liveTail = null;
    liveText = '';
    liveSettled = 0;
    // One final render over the whole text, replacing both halves. Whatever was
    // painted early was painted from a prefix; this is painted from the answer.
    follow(() => {
      bubble.classList.remove('streaming');
      bubble.replaceChildren(window.tshellMarkdown.render(text, copyStrings));
    });
    return true;
  }

  /** Throws away a partly-written answer, for when it is about to be written again. */
  function dropDelta() {
    stopLive();
    if (!liveBubble) return;
    const row = stepOf(liveBubble);
    if (row) row.remove();
    liveBubble = null;
    liveDone = null;
    liveTail = null;
    liveText = '';
    liveSettled = 0;
  }

  // -- thinking -------------------------------------------------------------

  /**
   * The live "still working" line, and the clock behind it.
   *
   * A reasoning model can spend minutes on one step, drawing nothing. What it is
   * thinking is behind a setting and off by default; that it is thinking, for how
   * long, and how much of it there has been so far is not optional -- that is the
   * difference between a slow panel and a dead one, and it costs one line.
   */
  let think = null;
  let thinkTimer = null;
  /*
   * The clock belongs to the step, not to the row that displays it.
   *
   * The row comes and goes inside one step -- it is dismissed the moment the
   * answer starts arriving, and a model that interleaves thinking with content
   * brings it back afterwards. Keeping the start time on the row meant every one
   * of those restarted the stopwatch, so a step that had been thinking for
   * fifteen seconds flashed up as 0s just before it finished. `ended` is what
   * stops it coming back at all once the step has moved on to writing.
   */
  let thinkStart = 0;
  let thinkTokens = 0;
  let thinkEnded = true;

  /** @param until When it stopped, or undefined while it is still going. */
  function elapsed(since, until) {
    const secs = Math.round(((until || Date.now()) - since) / 1000);
    if (secs < 60) return secs + 's';
    return Math.floor(secs / 60) + 'm' + String(secs % 60).padStart(2, '0') + 's';
  }

  /** A new step is about to start thinking, whether or not it will be shown. */
  function beginThinking() {
    thinkStart = Date.now();
    thinkTokens = 0;
    thinkEnded = false;
  }

  function paintThinking() {
    if (!think) return;
    const bits = [elapsed(thinkStart)];
    if (thinkTokens) bits.push(S.agentReasoningCount.replace('{0}', compact(thinkTokens)));
    think.meta.textContent = ' ' + bits.join(' · ');
  }

  function startThinking() {
    if (think || thinkEnded) return;
    const el = document.createElement('div');
    el.className = 'note thinking';
    el.append(document.createTextNode(S.agentThinking));
    const meta = document.createElement('span');
    meta.className = 'think-meta';
    el.append(meta);
    think = { el: el, meta: meta };
    paintThinking();
    appendStep(el);
    thinkTimer = setInterval(paintThinking, 1000);
  }

  function clearThinking() {
    if (thinkTimer) { clearInterval(thinkTimer); thinkTimer = null; }
    // The row, not just the note inside it. Removing only the note leaves an
    // empty wrapper on the timeline, still drawing its dot and its rail -- which
    // is exactly the spare dot that used to appear above every first reply.
    const row = stepOf(think && think.el);
    if (row) row.remove();
    think = null;
    thinkEnded = true;
  }

  // -- reasoning ------------------------------------------------------------

  /** The open thinking card, while the step it belongs to is still thinking. */
  let reason = null;
  let reasonTimer = null;

  /**
   * Puts the thought into the card. Takes the card's own state rather than
   * reading the open one, because a card outlives the step that wrote it: the
   * reader can expand it half an hour later, when `reason` is long since another
   * step's or nothing at all.
   */
  function fillReason(state) {
    const box = state.body;
    /*
     * Follows the thought only while the reader is already at the end of it.
     * Someone who has scrolled up inside an open card is reading something, and
     * yanking them back down every time another fragment lands makes the card
     * impossible to read at exactly the moment it has something worth reading.
     * Sampled before the change, because afterwards the answer is always no.
     */
    const stick = box.scrollTop + box.clientHeight >= box.scrollHeight - 24;
    // No colour on thinking. What is in a fence here is usually a draft the model
    // is still arguing with itself about, and syntax colour reads as a verdict on
    // code that has not been written yet -- the answer below is where that
    // belongs. It also costs a full highlight pass on text that grows by the
    // fragment and is repainted every time.
    box.replaceChildren(window.tshellMarkdown.render(state.text, copyStrings, { highlight: false }));
    if (stick) box.scrollTop = box.scrollHeight;
  }

  function drawReason(state) {
    /*
     * Always, and both figures: a card nobody has opened exists to say that the
     * step is alive and how much of it there has been, and one number that stops
     * moving while the other keeps going is how you tell thinking from stalled.
     * Frozen at `state.done` once the step ends, so the card reports how long it
     * took rather than how long ago it was.
     */
    state.count.textContent = elapsed(state.start, state.done)
      + ' · ' + S.agentReasoningCount.replace('{0}', compact(state.tokens));
    // The body is not on screen while the card is shut, and a step can think for
    // tens of thousands of tokens -- rebuilding that text node every repaint to
    // show none of it is the most expensive thing this panel could do. Reading
    // the scroll geometry costs a layout too, so a shut card gets neither. The
    // line above cannot change the thread's height either way: it is one short
    // line of text.
    if (!state.card.open) return;
    // An open card grows the thread until it reaches its own scroll height, so
    // the end of the thread has to be kept in view for that much of it.
    followLater(() => fillReason(state));
  }

  function paintReason() {
    reasonTimer = null;
    if (reason) drawReason(reason);
  }

  /*
   * Folded away by default and live all the same. The count on the summary is the
   * point of it: a step that thinks for two minutes says nothing and draws nothing,
   * and a number ticking over is the difference between working and hung. Opening
   * it is for when you want to know what it is working on.
   */
  function appendReasoning(text) {
    countOutput(text);
    /*
     * With the card switched off the thinking still counts, it just does not get
     * written down: the running line above picks up the tokens and goes on
     * reporting how long the step has been at it. Nothing is thrown away that was
     * ever going to be shown -- the reply is what gets kept either way.
     */
    if (!showReasoning) {
      // Counted whether or not anything is showing it, so the figure is right
      // if the row is still up and right again if the step brings it back.
      thinkTokens += estimateTokens(text);
      // Drawn by the clock already ticking once a second. Painting per fragment
      // would be dozens of DOM writes a second for a number nobody can read that
      // fast, which is how this panel got itself into trouble once.
      startThinking();
      return;
    }

    if (!reason) {
      clearThinking();
      const card = document.createElement('details');
      // Grey on the rail: it changed nothing on the machine. No chevron either --
      // the row opens because it is a summary, not because an arrow says so.
      card.className = 'reason';
      const head = document.createElement('summary');
      head.className = 'reason-head';
      const label = document.createElement('span');
      label.className = 'reason-label';
      label.textContent = S.agentReasoning;
      const count = document.createElement('span');
      count.className = 'reason-count';
      head.append(label, count);
      const body = document.createElement('div');
      body.className = 'reason-body';
      card.append(head, body);
      const state = { card: card, body: body, count: count, text: '', tokens: 0, start: Date.now(), done: 0 };
      // Filled on open, because a shut card is never filled: this is what puts
      // the thought there, whether the step is still running or ended long ago.
      card.addEventListener('toggle', () => { if (card.open) fillReason(state); });
      reason = state;
      appendStep(card);
    }
    reason.text += text;
    reason.tokens += estimateTokens(text);
    if (!reasonTimer) reasonTimer = setTimeout(paintReason, reason.text.length > 20000 ? 400 : 150);
  }

  /** Leaves the card where it is, finished. The next step opens its own. */
  function settleReasoning() {
    if (reasonTimer) { clearTimeout(reasonTimer); reasonTimer = null; }
    if (!reason) return;
    // Stops the clock before the last paint, so the card keeps the figure it
    // ended on rather than one that goes on climbing after the step is over.
    reason.done = Date.now();
    drawReason(reason);
    reason.card.classList.add('done');
    reason = null;
  }

  function dropReasoning() {
    if (reasonTimer) { clearTimeout(reasonTimer); reasonTimer = null; }
    const row = stepOf(reason && reason.card);
    if (row) row.remove();
    reason = null;
  }

  // -- usage ----------------------------------------------------------------

  /*
   * Thousands and millions, because a long task reaches both. The thresholds sit
   * just under the round numbers rather than on them, so a value that rounds up
   * into the next unit changes unit with it: 999,999 reads as 1M and never 1000k.
   */
  function compact(value) {
    if (value >= 999500) return (value / 1000000).toFixed(value >= 9999500 ? 0 : 1).replace(/\.0$/, '') + 'M';
    if (value >= 999.5) return (value / 1000).toFixed(value >= 9999.5 ? 0 : 1).replace(/\.0$/, '') + 'k';
    return String(Math.round(value));
  }

  /*
   * What a piece of text will have cost, near enough to watch.
   *
   * The real number only arrives with the last frame of the request, which for a
   * step that runs for a minute is a minute of a counter that does not move. So
   * output is counted as it arrives and the guess is replaced by the true figure
   * the moment the endpoint reports one. CJK runs close to a token a character;
   * most other scripts run nearer four characters to the token.
   */
  const CJK = /[ᄀ-ᇿ⺀-鿿ꥠ-꥿가-퟿豈-﫿︰-﹏＀-ﾟ]/;

  function estimateTokens(text) {
    let wide = 0;
    for (const char of text) if (CJK.test(char)) wide += 1;
    return Math.ceil(wide + (text.length - wide) / 4);
  }

  /**
   * Totals reported by the endpoint, and the guess riding on top of the latest
   * step. `total` stays null until something is actually reported, which is what
   * keeps the header blank for a provider that reports nothing rather than
   * showing it a confident zero. The round starts at zero because it is counting
   * from the moment the user pressed send, reported or not.
   */
  let total = null;
  let round = { prompt: 0, completion: 0, requests: 0 };
  let liveTokens = 0;

  /*
   * Counted on every fragment, drawn on a timer. Fragments arrive dozens of times
   * a second and the counters are three short strings, so painting each one is a
   * few hundred pointless DOM writes a second competing with the thread for the
   * same frames. Twice a second is faster than anyone reads a number anyway.
   */
  let usageTimer = null;

  function countOutput(text) {
    liveTokens += estimateTokens(text);
    if (!usageTimer) usageTimer = setTimeout(() => { usageTimer = null; paintUsage(); }, 500);
  }

  /**
   * `↑in ↓out`.
   *
   * Some of it is measured and some of it is this client's arithmetic -- a step
   * the endpoint never billed, or the step still arriving. That caveat lives in
   * the tooltip rather than in a symbol beside the number: it is true of most
   * conversations, so a mark that is almost always on is not a mark.
   */
  function usageText(usage, live) {
    return '↑' + compact(usage.prompt) + ' ↓' + compact(usage.completion + live);
  }

  /*
   * Both figures on one line at the foot of the box: what this message is costing
   * and what the conversation has cost. They sit where the eye already goes when
   * it leaves the thread, which is why neither needs a home of its own anywhere
   * else on screen.
   */
  function paintUsage() {
    const parts = [];
    if (round.requests || liveTokens) parts.push(S.agentUsageRound.replace('{0}', usageText(round, liveTokens)));
    if (total && total.requests) parts.push(S.agentUsageAll.replace('{0}', usageText(total, liveTokens)));
    const box = $('composerUsage');
    box.textContent = parts.join('  ·  ');
    const guessed = liveTokens > 0 || (round && round.estimated) || (total && total.estimated);
    box.title = parts.length && guessed ? S.agentUsageEstimated : '';
  }

  /**
   * How much conversation the next message will carry, in characters.
   *
   * Characters rather than tokens because that is what is actually known: tokens
   * would be this client's guess at someone else's tokeniser, and a guess dressed
   * as a count is worse than the plain measurement. Hidden at zero -- a fresh
   * conversation carries nothing, and a `0` on the toolbar is a thing to wonder
   * about rather than a thing to read.
   */
  function paintContext(chars) {
    const box = $('composerContext');
    box.textContent = chars ? S.agentContext.replace('{0}', compact(chars)) : '';
    box.hidden = !chars;
  }

  /**
   * Says how the last answer arrived, because the panel cannot show it any other
   * way and the reader keeps having to guess.
   *
   * Three outcomes, and the middle one is the reason this exists at all. An
   * endpoint that ignored `stream` is obvious once stated. An endpoint that
   * returned a real event stream carrying the whole answer in one or two frames
   * -- a proxy buffering the body -- is streaming by every check the client can
   * make and by nothing the reader can see, and it is the case people spend an
   * afternoon on. Anything else is working, and says so with its first-token
   * time, which is the number that tells a slow model from a slow link.
   */
  function paintTransport(info) {
    const box = $('composerTransport');
    const buffered = info.streamed && info.frames <= 2;
    const key = !info.streamed ? 'agentTransportWhole' : buffered ? 'agentTransportBuffered' : 'agentTransportStreamed';
    box.textContent = S[key]
      .replace('{0}', info.frames)
      .replace('{1}', (info.firstMs / 1000).toFixed(1));
    box.classList.toggle('warn', !info.streamed || buffered);
    box.hidden = false;
  }

  function setUsage(next, nextRound) {
    if (usageTimer) { clearTimeout(usageTimer); usageTimer = null; }
    total = next || null;
    if (nextRound) round = nextRound;
    // The step just reported is billed, so the guess standing in for it retires.
    liveTokens = 0;
    paintUsage();
  }

  // -- the composer ---------------------------------------------------------

  /*
   * Grows upward with what is typed, to a ceiling that still leaves the thread
   * worth reading. The box taking room from the thread is the same event as the
   * thread gaining a line, so the end is kept in view the same way -- decided
   * before the resize, because afterwards the answer is always no.
   */
  function autoGrow() {
    const input = $('input');
    const stick = atBottom();
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 200) + 'px';
    if (stick) thread.scrollTop = thread.scrollHeight;
  }

  /** Send while idle, stop while running: one button, two moments. */
  function setRunning(running) {
    const button = $('send');
    button.classList.toggle('running', running);
    button.title = S[running ? 'agentStop' : 'agentSend'] || '';
    button.setAttribute('aria-label', button.title);
    $('submitIcon').setAttribute('href', running ? '#i-stop' : '#i-send');
    if (!running) button.disabled = !$('input').value.trim();
    else button.disabled = false;
  }

  /**
   * A command and everything it produced live in one collapsible card. An
   * automatic read-only step starts closed, because the interesting thing is
   * usually the conclusion, not the twelve commands that led to it. Anything
   * that needs the user starts open, since its buttons have to be reachable.
   */
  function commandCard(command, why, verdict, unconfirmed) {
    const card = document.createElement('details');
    // The dot on the rail is what says which of the three this was, so the badge
    // that used to say it in words is gone and so is the chevron beside it.
    card.className = 'card';
    card.open = verdict !== 'auto';

    const head = document.createElement('summary');
    head.className = 'card-head';

    const reason = document.createElement('span');
    reason.className = 'card-why';
    reason.textContent = why || command;
    head.append(reason);

    /*
     * The one badge that came back.
     *
     * The others went because the rail already said what they said. This one it
     * cannot: a step that was going to be confirmed and then was not looks
     * exactly like one the user approved, and the difference is the whole of
     * what the mode did. So it is written out, in the two words it takes.
     */
    if (unconfirmed) {
      const badge = document.createElement('span');
      badge.className = 'card-badge';
      badge.textContent = unconfirmed === 'trusted' ? S.agentUnconfirmedTrusted : S.agentUnconfirmedAuto;
      head.append(badge);
    }
    /*
     * Kept for `addResult`, which otherwise folds a step away the moment it
     * succeeds. That was right while every one of these had been through a
     * dialog first -- the card was a second look at something already seen. With
     * the dialog skipped it is the only look there is, and folding it means the
     * change appeared and vanished in the same frame.
     */
    card.__unconfirmed = Boolean(unconfirmed);

    const code = document.createElement('code');
    code.className = 'command';
    code.textContent = command;

    card.append(head, code);
    /*
     * Three tones, because a refusal is not a request. Green ran on its own,
     * orange is waiting on the user, red was turned down and never ran -- the
     * distinction the badges used to draw in words, drawn in the one place the
     * eye already sweeps down.
     */
    return appendStep(
      card,
      verdict === 'auto' || unconfirmed ? 'ran' : verdict === 'refused' ? 'blocked' : 'needs'
    );
  }

  function addResult(card, result) {
    const failed = result.exitCode !== 0 || result.timedOut;

    const head = document.createElement('div');
    head.className = 'result-head';
    const label = document.createElement('span');
    label.className = 'exit' + (failed ? ' bad' : '');
    label.textContent = result.timedOut ? S.agentTimedOut : S.agentExit + ' ' + result.exitCode;
    head.append(label);
    if (result.truncated) {
      const extra = document.createElement('span');
      extra.textContent = '· ' + S.agentTruncated;
      head.append(extra);
    }

    const output = document.createElement('pre');
    output.className = 'output';
    output.textContent = result.output || '';

    follow(() => {
      /*
       * A finished step folds itself away. One that needed confirming was opened
       * so its buttons could be reached, and once it has run there is nothing left
       * to reach -- leaving it open buries the conversation under output nobody
       * asked to keep reading.
       *
       * Two exceptions, both the same exception: the step nobody has read yet.
       * A failure is worth reading because it is the one that did not work. A step
       * that ran without being confirmed is worth reading because the dialog that
       * would have shown it never opened -- fold it and the whole change appeared
       * and disappeared inside one frame, which is how this was found.
       *
       * Settling the card before the rows go in means it reaches its final height
       * in the layout it is inserted into, rather than growing afterwards.
       */
      card.open = failed || Boolean(card.__unconfirmed);
      card.append(head, output);
    });
  }

  // -- memory ---------------------------------------------------------------

  /** Label and tone per outcome. Anything unlisted reads as a plain failure. */
  const memoryOutcomes = {
    ok: { tone: 'ok' },
    duplicate: { label: 'memoryDuplicate', tone: 'muted' },
    full: { label: 'memoryFull', tone: 'warn' },
    missing: { label: 'memoryMissing', tone: 'muted' },
    ambiguous: { label: 'memoryAmbiguous', tone: 'muted' },
    failed: { label: 'memoryFailed', tone: 'bad' },
    oversize: { label: 'memoryOversize', tone: 'warn' },
    off: { label: 'memoryOff', tone: 'muted' },
    undone: { label: 'memoryUndone', tone: 'muted' }
  };

  /**
   * One line of memory, reported after the fact.
   *
   * Deliberately not a command card: nothing on the server changed and there is
   * no output to fold away, so it stays a single row the eye can skip. The undo
   * is offered only on a line this panel just wrote -- a replayed card is a record
   * of what happened, and its button would be pointing at a session that is over.
   */
  function memoryCard(event) {
    const state = memoryOutcomes[event.outcome] || memoryOutcomes.failed;
    const row = document.createElement('div');
    // Grey on the rail: writing a line to a local note changes nothing on the
    // machine, whatever the outcome tone says about how the write itself went.
    row.className = 'memory-row ' + state.tone;
    if (event.token) row.dataset.token = event.token;

    row.append(icon('#i-memory', 'icon memory-icon'));

    const label = document.createElement('span');
    label.className = 'memory-label';
    label.textContent = state.label
      ? S[state.label]
      : event.op === 'remember' ? S.memoryRemembered : S.memoryForgotten;

    const scope = document.createElement('span');
    scope.className = 'memory-scope';
    scope.textContent = event.scope === 'global' ? S.memoryScopeGlobal : S.memoryScopeServer;

    const text = document.createElement('span');
    text.className = 'memory-text';
    text.textContent = event.text;

    const actions = document.createElement('span');
    actions.className = 'memory-actions';
    if (!replaying && event.op === 'remember' && event.outcome === 'ok' && event.token) {
      const undo = document.createElement('button');
      undo.className = 'memory-action';
      undo.type = 'button';
      undo.textContent = S.memoryUndo;
      undo.onclick = () => { undo.disabled = true; post('undoMemory', { token: event.token }); };
      actions.append(undo);
    }
    const open = document.createElement('button');
    open.className = 'memory-action';
    open.type = 'button';
    open.textContent = S.memoryOpen;
    open.onclick = () => post('openMemory', { scope: event.scope });
    actions.append(open);

    row.append(label, scope, text, actions);
    return appendStep(row);
  }

  /**
   * The row that says a directory has been trusted.
   *
   * Deliberately the memory row's twin, down to the class names: both are the
   * assistant's behaviour changing for good on the strength of one answer, and
   * both are worth the same amount of the reader's attention. The difference is
   * the glyph and where the buttons go.
   */
  function trustedCard(event) {
    const row = document.createElement('div');
    // Grey on the rail, like memory: the machine was not touched by this.
    row.className = 'memory-row ok';

    row.append(icon('#i-trust', 'icon memory-icon'));

    const label = document.createElement('span');
    label.className = 'memory-label';
    label.textContent = S.trustAdded;

    const text = document.createElement('span');
    text.className = 'memory-text';
    text.textContent = event.dir;

    const actions = document.createElement('span');
    actions.className = 'memory-actions';
    // Undoing is removing it, which is what the window's delete button does --
    // so this is that button, reachable at the moment it is wanted rather than
    // two clicks away in a list the user has no reason to be looking at yet.
    if (!replaying) {
      const undo = document.createElement('button');
      undo.className = 'memory-action';
      undo.type = 'button';
      undo.textContent = S.memoryUndo;
      undo.onclick = () => { undo.disabled = true; post('removeTrust', { dir: event.dir }); };
      actions.append(undo);
    }
    const open = document.createElement('button');
    open.className = 'memory-action';
    open.type = 'button';
    open.textContent = S.memoryOpen;
    open.onclick = () => post('openTrust');
    actions.append(open);

    row.append(label, text, actions);
    return appendStep(row);
  }

  /**
   * Settles the card whose undo was pressed. A line that could not be found is
   * reported on the card rather than as an error: it is already not there, which
   * is what the button was for.
   */
  function markUndone(token, ok) {
    const row = thread.querySelector('.memory-row[data-token="' + token + '"]');
    if (!row) return;
    // Safe to rewrite wholesale: the timeline lives on the wrapper outside this
    // row, which is the other reason the dot is not drawn by the row itself.
    row.className = 'memory-row muted' + (ok ? ' undone' : '');
    row.querySelector('.memory-label').textContent = ok ? S.memoryUndone : S.memoryGone;
    row.querySelectorAll('.memory-action').forEach((button) => {
      if (button.textContent === S.memoryUndo) button.remove();
    });
  }

  // -- skills ---------------------------------------------------------------

  /** Label and tone per outcome. Only `ok` is a success; the rest carry on regardless. */
  const skillOutcomes = {
    ok: { tone: 'ok' },
    repeat: { label: 'skillRepeat', tone: 'muted' },
    unknown: { label: 'skillUnknown', tone: 'warn' },
    disabled: { label: 'skillDisabled', tone: 'muted' },
    missing: { label: 'skillMissing', tone: 'warn' },
    unreadable: { label: 'skillUnreadable', tone: 'warn' },
    off: { label: 'skillOff', tone: 'muted' }
  };

  /**
   * One skill pulled into the conversation, reported after the fact.
   *
   * Built like a memory row and for the same reason: nothing on the machine
   * changed, there is no output to fold, and what happened fits on one line. It
   * is worth a row at all because the answers that follow are steered by it, and
   * a user reading the thread should be able to see where that steering came
   * from without wondering why the assistant suddenly knows the house rules.
   */
  function skillCard(event) {
    const state = skillOutcomes[event.outcome] || skillOutcomes.unreadable;
    const row = document.createElement('div');
    row.className = 'memory-row ' + state.tone;
    row.append(icon('#i-skill', 'icon memory-icon'));

    const label = document.createElement('span');
    label.className = 'memory-label';
    label.textContent = state.label ? S[state.label] : S.skillLoaded;

    const name = document.createElement('span');
    name.className = 'memory-scope';
    name.textContent = event.id;

    const text = document.createElement('span');
    text.className = 'memory-text';
    // The file only when it is not the main one: naming SKILL.md on every row
    // would be repeating the shape of the feature rather than saying anything.
    text.textContent = [
      event.file && event.file !== 'SKILL.md' ? event.file : '',
      event.truncated ? S.skillTruncated : ''
    ].filter(Boolean).join(' · ');

    const actions = document.createElement('span');
    actions.className = 'memory-actions';
    const open = document.createElement('button');
    open.className = 'memory-action';
    open.type = 'button';
    open.textContent = S.skillOpen;
    open.onclick = () => post('openSkill', { id: event.id });
    actions.append(open);

    row.append(label, name, text, actions);
    return appendStep(row);
  }

  // -- permission dialog ----------------------------------------------------

  /** The confirm currently on screen. The loop is sequential, so there is at most one. */
  let pending = null;
  let choice = 0;
  /** What each option on the current dialog answers, in the order they are drawn. */
  let answerKeys = ['yes', 'no'];

  function paintChoice() {
    const options = $('confirmOptions').children;
    for (let i = 0; i < options.length; i += 1) {
      options[i].setAttribute('aria-selected', String(i === choice));
    }
  }

  function kindLabel(kind) {
    return kind === 'write' ? S.agentFileWrite : kind === 'append' ? S.agentFileAppend : S.agentFileEdit;
  }

  function fileTitle(data) {
    return kindLabel(data.kind) + ' ' + data.path;
  }

  /*
   * A file about to be written is code, and reads like code once it is coloured.
   * The language comes from the name where the name says something and from the
   * content where it does not -- a shell script called `deploy` has no extension
   * to read. Unknown leaves it as plain text rather than guessing.
   *
   * innerHTML is safe here for the same reason it is in the markdown renderer:
   * highlightLine escapes every literal it emits, so what arrives is spans around
   * already-escaped content.
   */
  function fillCode(pre, text, path) {
    const content = String(text === undefined || text === null ? '' : text);
    const language = window.tshellHighlight.languageForFile(path, content);
    if (!language) {
      pre.textContent = content;
      return;
    }
    pre.innerHTML = window.tshellHighlight.highlightText(content, language);
  }

  /**
   * A code block with the file's own line numbers down its left edge.
   *
   * The numbers go in a `<pre>` of their own beside the code rather than being
   * threaded into it, because the code arrives as one blob of highlighted HTML
   * and a span in it may run across a newline -- a block comment, a multi-line
   * string. Splitting that by line to prefix each one would cut those spans in
   * half. Two elements side by side stay in step instead, on the strength of one
   * rule in the stylesheet: the code no longer wraps, so one line of it is
   * always exactly one row.
   *
   * `first` is the line the text starts on in the file, which for an edit is
   * where the fragment sits rather than 1. Left off, so is the gutter.
   */
  function codeBlock(text, path, first) {
    const body = document.createElement('div');
    body.className = 'file-body';

    const code = document.createElement('pre');
    code.className = 'file-code';
    fillCode(code, text, path);

    if (first) {
      const content = String(text === undefined || text === null ? '' : text);
      // One trailing newline is the file ending tidily, not a last empty line,
      // and `<pre>` does not draw a row for it either.
      const count = content.replace(/\n$/, '').split('\n').length;
      const gutter = document.createElement('pre');
      gutter.className = 'file-gutter';
      gutter.setAttribute('aria-hidden', 'true');
      const numbers = [];
      for (let i = 0; i < count; i += 1) numbers.push(first + i);
      gutter.textContent = numbers.join('\n');
      body.append(gutter);
    }

    body.append(code);
    return body;
  }

  function block(label, text, kind, path, first) {
    const part = document.createElement('div');
    part.className = 'file-part ' + kind;
    const head = document.createElement('div');
    head.className = 'file-part-label';
    head.textContent = label;
    // Both halves of a diff are code and are coloured as code. What tells them
    // apart is the tint behind them, not the colour of the text, which is why
    // the stylesheet no longer paints either side red or green.
    part.append(head, codeBlock(text, path, first));
    return part;
  }

  /**
   * Draws the change itself. A write shows the file as it will be; an edit shows
   * the fragment coming out and the one going in, which is the only way to judge
   * whether the right occurrence was found.
   *
   * The same view serves the prompt and the thread: what the user agreed to and
   * what the thread reports afterwards should not be two different drawings of
   * the same change. The prompt leaves out the path, having it in its title.
   */
  function fileView(data, withPath) {
    const view = document.createElement('div');
    // The kind rides on the element so the stylesheet can tint new content green
    // without the script having to know what green means.
    view.className = 'file-view ' + data.kind;

    const meta = document.createElement('div');
    meta.className = 'file-meta';
    const state = data.kind === 'append' ? '' : data.exists ? S.agentFileReplace : S.agentFileNew;
    // Only when it is not UTF-8: that a file keeps a legacy encoding is news, and
    // the user is the one who knows whether that is still what they want.
    const encoding = data.encoding && data.encoding !== 'utf8' ? 'GBK' : '';
    const parts = withPath
      ? [kindLabel(data.kind), data.path, state, encoding, data.size]
      : [state, encoding, data.size];
    meta.textContent = parts.filter(Boolean).join(' · ');
    view.append(meta);

    if (data.kind === 'edit') {
      // Both halves start on the same line: everything above the match is
      // untouched, so what is coming out and what is going in begin together.
      view.append(
        block(S.agentFileRemoved, data.before, 'del', data.path, data.line),
        block(S.agentFileAdded, data.after, 'add', data.path, data.line)
      );
      return view;
    }
    view.append(codeBlock(data.preview, data.path, data.line));
    return view;
  }

  function paintFile(data) {
    const body = $('confirmBody');
    body.hidden = !data;
    body.replaceChildren();
    if (data) body.append(fileView(data, false));
  }

  /** The change stays in the card, so it can be read again long after the prompt. */
  function addFile(card, data) {
    follow(() => {
      card.open = true;
      card.append(fileView(data, true));
    });
  }

  // -- transfers ------------------------------------------------------------

  const sizeUnits = ['B', 'KB', 'MB', 'GB'];

  function size(bytes) {
    let value = bytes || 0;
    let unit = 0;
    while (value >= 1024 && unit < sizeUnits.length - 1) { value /= 1024; unit += 1; }
    return (unit === 0 ? value : value.toFixed(1)) + ' ' + sizeUnits[unit];
  }

  function percent(done, total) {
    return (total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0) + '%';
  }

  function progressRow() {
    const line = document.createElement('div');
    line.className = 'transfer-row';
    const label = document.createElement('span');
    label.className = 'transfer-label';
    const stat = document.createElement('span');
    stat.className = 'transfer-stat';
    const track = document.createElement('div');
    track.className = 'bar';
    track.append(document.createElement('i'));
    line.append(label, stat, track);
    return line;
  }

  /**
   * What a transfer is moving and where it is going, with the two bars the
   * progress ticks fill in: everything, and the file currently in flight.
   *
   * Drawn once and then repainted in place. Appending a row per tick would push
   * the thread down several times a second, and the interesting thing about a
   * transfer is where it has got to, not the trail of where it has been.
   *
   * A replayed conversation reaches this with no ticks to follow -- they are
   * deliberately not recorded -- so the bars stay hidden and the view is the one
   * line that still means something afterwards: what went where.
   */
  function transferView(data) {
    const view = document.createElement('div');
    view.className = 'transfer-view';

    const meta = document.createElement('div');
    meta.className = 'transfer-meta';
    meta.textContent = (data.kind === 'upload' ? S.agentUpload : S.agentDownload)
      + ' ' + (data.sources || []).join(', ') + ' → ' + data.target;

    const bars = document.createElement('div');
    bars.className = 'transfer-bars';
    bars.hidden = true;
    bars.append(progressRow(), progressRow());

    view.append(meta, bars);
    return view;
  }

  function paintProgress(view, progress) {
    const bars = view.querySelector('.transfer-bars');
    const rows = bars.children;
    const overall = progress.overall;
    const current = progress.current;
    const scanning = progress.phase === 'scanning';
    bars.hidden = false;

    // Scanning knows how much it has found so far but not how much there is, so
    // it reports the total it is building rather than a percentage it cannot know.
    rows[0].children[0].textContent = scanning
      ? S.agentTransferScanning
      : S.agentTransferFiles.replace('{0}', overall.doneFiles + '/' + overall.totalFiles);
    rows[0].children[1].textContent = scanning
      ? S.agentTransferFiles.replace('{0}', String(overall.totalFiles)) + ' · ' + size(overall.totalBytes)
      : size(overall.doneBytes) + ' / ' + size(overall.totalBytes);
    rows[0].children[2].firstChild.style.width = scanning ? '0%' : percent(overall.doneBytes, overall.totalBytes);

    rows[1].hidden = scanning || !current.name;
    if (rows[1].hidden) return;
    rows[1].children[0].textContent = current.name;
    rows[1].children[1].textContent = size(current.transferred) + ' / ' + size(current.total);
    rows[1].children[2].firstChild.style.width = percent(current.transferred, current.total);
  }

  /** Opens the card: a progress bar nobody can see is not progress. */
  function addTransfer(card, data) {
    return follow(() => {
      card.open = true;
      const view = transferView(data);
      card.append(view);
      return view;
    });
  }

  function askConfirm(data) {
    pending = data.id;
    choice = 0;
    const file = Boolean(data.kind);
    $('confirmTitle').textContent = file ? S.agentConfirmFileTitle : S.agentConfirmTitle;
    $('confirmCommand').textContent = file ? fileTitle(data) : data.command;
    $('confirmWhy').textContent = data.why || '';
    paintFile(file ? data : null);

    /*
     * Two answers, or three when there is a directory worth offering.
     *
     * The third is only ever on a file dialog, and only when the path was
     * absolute -- the extension side works out whether there is a directory this
     * could mean and sends it, so an answer that could not be honoured is never
     * on the screen. It sits second, between yes and no: it is a kind of yes,
     * and putting it last would make "no" the middle key on a dialog where the
     * middle key is the one people reach for without reading.
     */
    const answers = [{ label: file ? S.agentConfirmFileYes : S.agentConfirmYes, answer: 'yes' }];
    if (file && data.trustDir) answers.push({ label: S.agentConfirmFileTrust, answer: 'trust' });
    answers.push({ label: S.agentConfirmNo, answer: 'no' });

    const options = $('confirmOptions');
    options.replaceChildren();
    answers.forEach((entry, index) => {
      const option = document.createElement('button');
      option.className = 'dialog-option';
      option.type = 'button';
      const key = document.createElement('span');
      key.className = 'key';
      key.textContent = String(index + 1);
      option.append(key, document.createTextNode(entry.label));
      if (entry.answer === 'trust') option.title = data.trustDir;
      option.onmouseenter = () => { choice = index; paintChoice(); };
      option.onclick = () => answerConfirm(entry.answer);
      options.append(option);
    });
    answerKeys = answers.map((entry) => entry.answer);
    paintChoice();
    $('confirm').hidden = false;
    // Takes the keyboard off the composer, so Enter answers the prompt -- but
    // only if it was in this panel to begin with. See `takeFocus`.
    takeFocus(options.firstElementChild);
  }

  function answerConfirm(answer) {
    if (pending === null) return;
    const id = pending;
    closeConfirm();
    post('confirm', { id: id, answer: answer });
  }

  function closeConfirm() {
    pending = null;
    $('confirm').hidden = true;
    takeFocus($('input'));
  }

  /*
   * Captured on the document so the prompt answers the keyboard wherever focus
   * happens to be, and so Enter never reaches the composer behind the backdrop.
   */
  document.addEventListener('keydown', (event) => {
    if (pending === null) return;
    const count = answerKeys.length;
    // A number key answers the option carrying it, however many there are. Read
    // off the list rather than written out, so the third one is not a special
    // case that exists in the dialog and not on the keyboard.
    const digit = /^[1-9]$/.test(event.key) ? Number(event.key) - 1 : -1;
    switch (event.key) {
      case 'ArrowDown':
        choice = (choice + 1) % count;
        paintChoice();
        break;
      case 'ArrowUp':
        choice = (choice + count - 1) % count;
        paintChoice();
        break;
      case 'Enter':
        answerConfirm(answerKeys[choice]);
        break;
      // Dismissing is the safe answer, so Esc skips rather than leaving it open.
      case 'Escape':
        answerConfirm('no');
        break;
      default:
        if (digit < 0 || digit >= count) return;
        answerConfirm(answerKeys[digit]);
    }
    event.preventDefault();
    event.stopPropagation();
  }, true);

  // -- history --------------------------------------------------------------

  function openHistory() {
    openPanel('historyPanel', 'history');
  }

  function closeHistory() {
    closePanels();
  }

  /** Today is a time; anything older needs its date to mean anything. */
  function when(stamp) {
    const date = new Date(stamp);
    const time = date.toLocaleTimeString(boot.language, { hour: '2-digit', minute: '2-digit' });
    if (date.toDateString() === new Date().toDateString()) return time;
    return date.toLocaleDateString(boot.language) + ' ' + time;
  }

  function historyRow(item, current) {
    const row = document.createElement('div');
    row.className = 'history-row' + (item.id === current ? ' current' : '');

    const open = document.createElement('button');
    open.className = 'history-open';
    open.type = 'button';
    const title = document.createElement('div');
    title.className = 'history-title';
    title.textContent = item.title || item.serverName;
    const meta = document.createElement('div');
    meta.className = 'history-meta';
    meta.textContent = [item.serverName, when(item.updatedAt), item.id === current ? S.agentHistoryCurrent : '']
      .filter(Boolean).join(' · ');
    open.append(title, meta);
    open.onclick = () => { post('loadChat', { id: item.id }); closeHistory(); };

    const remove = document.createElement('button');
    remove.className = 'icon-button danger';
    remove.type = 'button';
    remove.title = S.agentHistoryDelete;
    remove.setAttribute('aria-label', S.agentHistoryDelete);
    remove.append(icon('#i-trash'));
    remove.onclick = () => post('deleteChat', { id: item.id });

    row.append(open, remove);
    return row;
  }

  function paintHistory(items, current) {
    const list = $('historyList');
    list.replaceChildren();
    $('historyEmpty').hidden = items.length > 0;
    items.forEach((item) => list.append(historyRow(item, current)));
  }

  // -- memory, skill and model windows --------------------------------------

  /*
   * Four overlays, one at a time.
   *
   * They are alternatives, not layers: each answers "what is this assistant
   * working from", and two of them open at once would be a stack of modals over
   * a thread nobody can see. So opening one closes the others, and Esc closes
   * whichever is up.
   */
  const panels = [
    'historyPanel', 'memoryPanel', 'skillPanel', 'thinkPanel', 'modePanel', 'trustPanel', 'modelPanel'
  ];

  function openPanel(id, request) {
    panels.forEach((name) => { $(name).hidden = name !== id; });
    if (request) post(request);
  }

  function closePanels() {
    panels.forEach((name) => { $(name).hidden = true; });
    $('modelForm').hidden = true;
    $('input').focus();
  }

  function openPanelId() {
    return panels.find((name) => !$(name).hidden);
  }

  /** A row with a body on the left and its actions on the right. */
  function panelRow(title, meta, extra) {
    const row = document.createElement('div');
    row.className = ('panel-row ' + (extra || '')).trim();

    const text = document.createElement('div');
    text.className = 'panel-text';
    const head = document.createElement('div');
    head.className = 'panel-title';
    head.textContent = title;
    text.append(head);
    if (meta) {
      const sub = document.createElement('div');
      sub.className = 'panel-meta';
      sub.textContent = meta;
      text.append(sub);
    }

    const buttons = document.createElement('div');
    buttons.className = 'panel-buttons';
    row.append(text, buttons);
    row.__text = text;
    row.__buttons = buttons;
    return row;
  }

  function panelButton(label, onclick, className) {
    const button = document.createElement('button');
    button.className = 'panel-action' + (className ? ' ' + className : '');
    button.type = 'button';
    button.textContent = label;
    button.onclick = onclick;
    return button;
  }

  /**
   * The same button with the glyph doing the talking.
   *
   * For the actions whose written label was longer than the thing it acted on.
   * The name still ships, as the tooltip and as what a screen reader reads: an
   * icon is shorthand for people who can see it, never the only copy.
   */
  function panelIcon(glyph, label, onclick, className) {
    const button = document.createElement('button');
    button.className = ('icon-button panel-icon ' + (className || '')).trim();
    button.type = 'button';
    button.title = label;
    button.setAttribute('aria-label', label);
    button.append(icon(glyph, 'icon'));
    button.onclick = onclick;
    return button;
  }

  /**
   * Turns a row's buttons into a yes/no for one destructive answer.
   *
   * Inline rather than a modal, which is what moving these menus into the panel
   * was for. The row is redrawn from the server's answer either way, so a "no"
   * needs nothing more than putting the buttons back.
   */
  function confirmInline(row, question, confirm, restore) {
    const ask = document.createElement('span');
    ask.className = 'panel-meta';
    ask.textContent = question;
    row.__buttons.replaceChildren(
      ask,
      panelButton(S.memoryDelete, confirm, 'danger'),
      panelButton(S.memoryCancel, restore)
    );
  }

  // -- memory ---------------------------------------------------------------

  /*
   * A note, and nothing else.
   *
   * Rewriting a line in place put three controls on every row of a window whose
   * whole job is to be read at a glance -- and it was a worse text editor than
   * the one already open behind it. So the row went back to being the note, and
   * the file is handed over whole from the heading above it.
   */
  function memoryRow(line) {
    return panelRow(line.replace(/^\s*[-*]\s*/, ''), '', 'note');
  }

  function paintMemory(data) {
    if (!data) return;
    const list = $('memoryList');
    list.replaceChildren();

    const notice = $('memoryNotice');
    if (!data.enabled) {
      notice.textContent = S.memoryOff;
      notice.hidden = false;
      return;
    }
    // Both scopes are drawn even when both are empty. A single "nothing yet"
    // across the whole window says less than two named files do, and it took the
    // headings with it -- and with them the only way in to either file, at the
    // one moment someone would want it.
    notice.hidden = true;

    // The machine's own notes first: that is the scope almost every line is in,
    // and the one the user came to look at.
    const groups = [
      { scope: 'server', label: data.serverName || S.memoryScopeServer, lines: data.server },
      { scope: 'global', label: S.memoryScopeGlobal, lines: data.global }
    ];
    groups.forEach((group) => {
      // A scope is a file, so its heading is that file's edge and carries the
      // one way in. Present even when the scope is empty: an empty file is
      // exactly when someone wants to open it and type.
      const head = document.createElement('div');
      head.className = 'panel-group';
      const label = document.createElement('span');
      label.textContent = group.label;
      head.append(
        label,
        panelIcon('#i-edit', S.memoryEdit, () => post('openMemory', { scope: group.scope }))
      );
      list.append(head);
      if (!group.lines.length) {
        const empty = document.createElement('div');
        empty.className = 'panel-meta panel-group-empty';
        empty.textContent = S.memoryEmpty;
        list.append(empty);
        return;
      }
      group.lines.forEach((line) => list.append(memoryRow(line)));
    });
  }

  // -- skills ---------------------------------------------------------------

  function skillRow(item) {
    const broken = !item.description;
    const row = panelRow(
      item.name,
      broken ? S.skillNoDescription : item.description,
      [item.disabled ? 'off' : '', broken ? 'broken' : ''].filter(Boolean).join(' ')
    );
    // Stays a word rather than becoming a glyph: this one switches a state, and
    // a state needs to say which one it puts you in.
    row.__buttons.append(
      panelButton(item.disabled ? S.skillEnable : S.skillDisable, () => post('toggleSkill', { id: item.id }))
    );
    return row;
  }

  function paintSkills(data) {
    const list = $('skillList');
    list.replaceChildren();
    const notice = $('skillNotice');
    if (!data.enabled) {
      notice.textContent = S.skillsOff;
      notice.hidden = false;
      return;
    }
    if (!data.items.length) {
      notice.textContent = S.skillEmpty;
      notice.hidden = false;
      return;
    }
    notice.hidden = true;
    data.items.forEach((item) => list.append(skillRow(item)));
  }

  // -- mode -----------------------------------------------------------------

  /**
   * The mode currently in force, mirrored here so the window can be drawn
   * without a round trip and so the toolbar glyph survives a redraw.
   */
  let currentMode = 'ask';

  const modeGlyphs = { ask: '#i-mode-ask', trust: '#i-mode-trust', auto: '#i-mode-auto' };

  function paintMode(mode) {
    currentMode = modeGlyphs[mode] ? mode : 'ask';
    $('modeIcon').setAttribute('href', modeGlyphs[currentMode]);
    // Auto is the one that can let something through unwatched, so it says so in
    // colour as well as in shape. The other two are the ordinary foreground.
    $('mode').classList.toggle('danger-on', currentMode === 'auto');
    if (!$('modePanel').hidden) paintModeList();
  }

  function paintModeList() {
    const list = $('modeList');
    list.replaceChildren();
    [
      { id: 'ask', label: S.modeAsk, hint: S.modeAskHint },
      { id: 'trust', label: S.modeTrust, hint: S.modeTrustHint },
      { id: 'auto', label: S.modeAuto, hint: S.modeAutoHint }
    ].forEach((entry) => {
      const row = panelRow(entry.label, entry.hint, entry.id === currentMode ? 'current' : '');
      row.__text.style.cursor = 'pointer';
      row.__text.onclick = () => chooseMode(entry.id);
      if (entry.id === 'auto') row.classList.add('mode-auto');
      list.append(row);
    });
  }

  /**
   * Picking a mode, with one of them asking again first.
   *
   * The second question is inline in the row rather than a modal on top of a
   * modal: the window is already the user's whole attention, and stacking two
   * dialogs to answer one question reads as a stutter rather than as gravity.
   */
  function chooseMode(mode) {
    if (mode === currentMode) { closePanels(); return; }
    if (mode !== 'auto') {
      post('setMode', { mode: mode });
      closePanels();
      return;
    }
    const list = $('modeList');
    list.replaceChildren();
    const warning = document.createElement('div');
    warning.className = 'mode-warning';
    const title = document.createElement('div');
    title.className = 'panel-title';
    title.textContent = S.modeAutoConfirmTitle;
    const body = document.createElement('div');
    body.className = 'panel-meta';
    body.textContent = S.modeAutoConfirmBody;
    const buttons = document.createElement('div');
    buttons.className = 'panel-buttons';
    buttons.append(
      panelButton(S.modeAutoConfirmYes, () => { post('setMode', { mode: 'auto' }); closePanels(); }, 'danger'),
      panelButton(S.modeAutoConfirmNo, paintModeList)
    );
    warning.append(title, body, buttons);
    list.append(warning);
  }

  // -- thinking --------------------------------------------------------------

  /**
   * What the model is asked to do, and what this panel does with what comes back.
   *
   * Drawn from `aiState` like the mode is, so the window opens with no round trip
   * and every panel shows the same thing a moment after any of them changes it.
   */
  let thinkState = { enabled: true, effort: 'high', show: false };

  function applyThinkState(next) {
    thinkState = {
      enabled: next.enabled !== false,
      effort: next.effort === 'low' || next.effort === 'max' ? next.effort : 'high',
      show: next.show === true
    };
    showReasoning = thinkState.show;
    // Lit means the model is thinking. That is the state worth reading off a
    // toolbar -- it is what a step costs and how long it takes -- while whether
    // the thinking is drawn is a question about this panel and lives inside.
    $('think').setAttribute('aria-pressed', String(thinkState.enabled));
    if (!$('thinkPanel').hidden) paintThinkList();
  }

  /**
   * One question, answered by picking one of two or three words.
   *
   * The same control for all three settings here, which is the point: what the
   * window has to make obvious is that the top one governs the two below it, and
   * a dependency reads at a glance only when the things it links look alike.
   * Real buttons rather than a styled div, so Tab reaches them, Space works, and
   * `disabled` means what it says to a screen reader as well as to a mouse.
   */
  function segmented(label, options, current, pick, disabled) {
    const group = document.createElement('div');
    group.className = 'c-seg';
    group.setAttribute('role', 'group');
    group.setAttribute('aria-label', label);
    options.forEach((option) => {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'c-seg-item' + (option.id === current ? ' c-on' : '');
      button.textContent = option.label;
      button.setAttribute('aria-pressed', String(option.id === current));
      button.disabled = disabled === true;
      button.onclick = () => pick(option.id);
      group.append(button);
    });
    return group;
  }

  function paintThinkList() {
    const list = $('thinkList');
    list.replaceChildren();
    /*
     * Greyed rather than hidden. A row that vanishes takes its own explanation
     * with it -- the reason these two are unavailable is the switch directly
     * above them, and that only reads while all three are on screen. Their
     * values are kept, so turning thinking back on returns to what was chosen.
     */
    const off = !thinkState.enabled;

    const power = panelRow(S.thinkEnabled, S.thinkEnabledHint);
    power.__buttons.append(segmented(S.thinkEnabled, [
      { id: 'on', label: S.thinkOn },
      { id: 'off', label: S.thinkOff }
    ], off ? 'off' : 'on', (id) => post('setThinking', { enabled: id === 'on' })));
    list.append(power);

    const effort = panelRow(S.thinkEffort, S.thinkEffortNote, off ? 'off' : '');
    effort.__buttons.append(segmented(S.thinkEffort, [
      { id: 'low', label: S.thinkEffortLow },
      { id: 'high', label: S.thinkEffortHigh },
      { id: 'max', label: S.thinkEffortMax }
    ], thinkState.effort, (id) => post('setThinking', { effort: id }), off));
    list.append(effort);

    const show = panelRow(S.thinkShow, S.thinkShowHint, off ? 'off' : '');
    show.__buttons.append(segmented(S.thinkShow, [
      { id: 'yes', label: S.thinkYes },
      { id: 'no', label: S.thinkNo }
    ], thinkState.show ? 'yes' : 'no', (id) => post('setThinking', { show: id === 'yes' }), off));
    list.append(show);
  }

  // -- trusted directories ---------------------------------------------------

  function trustRow(dir) {
    const row = panelRow(dir, '', 'note');
    const restore = () => {
      row.__buttons.replaceChildren(panelIcon('#i-trash', S.trustDelete, remove, 'danger'));
    };
    const remove = () => confirmInline(row, '', () => post('removeTrust', { dir: dir }), restore);
    restore();
    return row;
  }

  function paintTrust(data) {
    const list = $('trustList');
    list.replaceChildren();
    const notice = $('trustNotice');
    if (!data.dirs.length) {
      notice.textContent = S.trustEmpty;
      notice.hidden = false;
      return;
    }
    // A list that is on screen but decides nothing is worth saying out loud --
    // otherwise "I trusted it and it still asked me" looks like a bug.
    notice.textContent = data.mode === 'ask' ? S.trustInert : '';
    notice.hidden = data.mode !== 'ask';
    data.dirs.forEach((dir) => list.append(trustRow(dir)));
  }

  // -- models ---------------------------------------------------------------

  function modelRow(item, active) {
    const row = panelRow(
      item.label,
      [item.detail, item.hasKey ? S.agentModelKeySet : '', item.id === active ? S.agentModelCurrent : '']
        .filter(Boolean).join(' · '),
      item.id === active ? 'current' : ''
    );
    // The text is the switch. Deleting is the button, so the common action needs
    // no aim and the uncommon one cannot be hit by accident.
    row.__text.style.cursor = 'pointer';
    row.__text.onclick = () => post('selectModel', { id: item.id });

    if (item.removable) {
      const restore = () => {
        row.__buttons.replaceChildren(panelIcon('#i-trash', S.agentModelDelete, remove, 'danger'));
      };
      const remove = () => confirmInline(
        row,
        S.agentModelConfirmDeleteShort,
        () => post('deleteModel', { id: item.id }),
        restore
      );
      restore();
    }
    return row;
  }

  function paintModels(data) {
    const list = $('modelList');
    list.replaceChildren();
    data.items.forEach((item) => list.append(modelRow(item, data.active)));
  }

  function submitModel() {
    const baseUrl = $('modelBaseUrl').value.trim();
    const model = $('modelName').value.trim();
    if (!baseUrl || !model) return;
    post('addModel', { baseUrl: baseUrl, model: model, apiKey: $('modelKey').value });
    // Wiped the moment it is sent. The key has no reason to stay in the DOM,
    // and this panel is retained when hidden.
    $('modelBaseUrl').value = '';
    $('modelName').value = '';
    $('modelKey').value = '';
    $('modelForm').hidden = true;
  }

  // -- events ---------------------------------------------------------------

  /** True while a stored conversation is being redrawn, which suppresses undo. */
  let replaying = false;

  function clearThread() {
    thread.querySelectorAll('.step, .turn').forEach((el) => el.remove());
    openCard = null;
    openTransfer = null;
    // The nodes these pointed at have just been removed with everything else.
    clearThinking();
    stopLive();
    liveBubble = null;
    liveDone = null;
    liveTail = null;
    liveText = '';
    liveSettled = 0;
    dropReasoning();
    // It describes the last request of the conversation being cleared away, and
    // there has not been one in the new conversation yet.
    $('composerTransport').hidden = true;
    $('empty').hidden = false;
  }

  /**
   * Puts a recorded conversation back on screen by replaying what drew it the
   * first time. The events are the same ones the panel handles live, so there is
   * no second rendering path to keep in step with this one.
   */
  function replay(entries) {
    clearThread();
    replaying = true;
    try {
      entries.forEach((entry) => {
        if (entry.kind === 'user') turn(entry.text, 'user');
        else if (entry.event) handleEvent(entry.event);
      });
    } finally {
      replaying = false;
    }
    setRunning(false);
  }

  function handleEvent(event) {
    switch (event.type) {
      case 'thinking':
        // Anything still half-written belongs to an attempt that did not finish:
        // the request failed, or came back empty and is being made again. What
        // replaces it is about to be streamed from the start.
        dropDelta();
        settleReasoning();
        // A replayed thread is a record, not a run: nothing in it is still thinking.
        if (replaying) break;
        /*
         * With the thinking card switched on, the card is the running indicator
         * and this line would only be a second one saying the same thing a beat
         * earlier. Off, it is the only thing there is, so it stays.
         */
        beginThinking();
        if (!showReasoning) startThinking();
        setRunning(true);
        break;
      case 'delta':
        appendDelta(event.text);
        break;
      case 'reasoning':
        appendReasoning(event.text);
        break;
      case 'retry':
        // Both belong to the attempt that failed. What replaces them is about to
        // arrive from the beginning.
        dropDelta();
        dropReasoning();
        clearThinking();
        // A reply that never came is not a connection that dropped, and the card
        // just cleared away was a page of thinking. Saying which happened is the
        // difference between a pause with a reason and a panel that flickered.
        note(event.kind === 'empty'
          ? S.agentRetryingEmpty
          : S.agentRetrying.replace('{0}', event.attempt), 'retry');
        break;
      case 'transport':
        paintTransport(event);
        break;
      case 'context':
        paintContext(event.chars);
        break;
      /*
       * A setting that did not reach the endpoint. Drawn as a warning rather
       * than a passing note: the answer under it is real, and it was produced
       * under settings other than the ones the toolbar is showing, which is
       * exactly the kind of thing that is never noticed unless it is said.
       */
      case 'degraded':
        note(S.thinkDegraded.replace('{0}', event.fields.join(', ')), 'warn');
        break;
      case 'command':
        clearThinking();
        // The model chose to act rather than answer, so nothing was streamed.
        dropDelta();
        settleReasoning();
        openCard = commandCard(event.command, event.why, event.verdict, event.unconfirmed);
        openTransfer = null;
        break;
      case 'refused':
        clearThinking();
        openCard = null;
        commandCard(event.command, (event.reasons || []).join(' · '), 'refused');
        break;
      case 'declined':
        note(S.agentDeclined);
        openCard = null;
        break;
      case 'memory':
        clearThinking();
        // Its own row, so it never lands inside the card of a command that
        // happened to be open when the model decided to write something down.
        openCard = null;
        memoryCard(event);
        break;
      case 'trusted':
        clearThinking();
        openCard = null;
        trustedCard(event);
        break;
      case 'skill':
        clearThinking();
        // Same reasoning as memory: a row of its own, never folded into whatever
        // command card happened to be open when the model reached for a skill.
        openCard = null;
        skillCard(event);
        break;
      case 'reply':
        clearThinking();
        settleReasoning();
        // The bubble the answer was streamed into is already on screen; it only
        // needs rendering once more, on the whole text rather than on a prefix.
        if (!settleDelta(event.text)) turn(event.text, 'assistant');
        break;
      case 'file':
        if (openCard) addFile(openCard, event);
        break;
      case 'transfer':
        if (openCard) openTransfer = addTransfer(openCard, event);
        break;
      case 'transferProgress':
        if (openTransfer) follow(() => paintProgress(openTransfer, event.progress));
        break;
      case 'result':
        if (openCard) addResult(openCard, event);
        openCard = null;
        openTransfer = null;
        break;
      // A question and a wrap-up are both just the assistant talking, and both
      // stream the same way an ordinary reply does.
      case 'question':
        clearThinking();
        settleReasoning();
        if (!settleDelta(event.question)) turn(event.question, 'assistant');
        break;
      case 'summary':
        clearThinking();
        settleReasoning();
        if (!settleDelta(event.summary)) turn(event.summary, 'assistant');
        break;
      case 'stepLimit':
        clearThinking();
        note(S.agentStepLimit.replace('{0}', event.steps));
        break;
      case 'stopped':
        clearThinking();
        // Half an answer to a request nobody is waiting for any more.
        dropDelta();
        settleReasoning();
        note(S.agentStopped);
        break;
      case 'error':
        clearThinking();
        dropDelta();
        settleReasoning();
        // A coded failure has a string in the reader's language; anything else
        // carries its own text, which is the only thing there is to show.
        note((event.code && S['agent' + event.code[0].toUpperCase() + event.code.slice(1)]) || event.message, 'error');
        break;
      case 'idle':
        // First, before anything that touches the thread. Everything else in
        // this branch is tidying; this one is the difference between the user
        // being able to send their next message and not.
        setRunning(false);
        clearThinking();
        settleReasoning();
        // The in-flight guess has been replaced by the step's own figure by now,
        // so what is left of it is double counting.
        liveTokens = 0;
        paintUsage();
        break;
    }
  }

  window.addEventListener('message', (message) => {
    const data = message.data;
    if (data.type === 'event') {
      /*
       * One event that cannot be drawn must not take the rest of the task with
       * it. Every branch below leaves panel state behind -- which card is open,
       * whether a task is running -- and letting a rendering fault propagate out
       * of here is how a thread stops responding to a task that is going fine.
       */
      try {
        handleEvent(data.event);
      } catch (error) {
        console.error('tshell: could not draw event', data.event && data.event.type, error);
      }
      return;
    }
    if (data.type === 'aiState') {
      applyAiState(data);
      return;
    }
    // Sent when the figure settles -- a conversation opened, cleared or finished.
    // While one is running it arrives as an event instead, step by step.
    if (data.type === 'context') {
      paintContext(data.chars);
      return;
    }
    if (data.type === 'user') {
      turn(data.text, 'user');
      // A new message is a new round, counted from here whether or not the first
      // step has reported anything yet.
      round = { prompt: 0, completion: 0, requests: 0 };
      liveTokens = 0;
      paintUsage();
      return;
    }
    if (data.type === 'confirmFile') {
      // The loop draws the card before it reads the file, so one is normally
      // already open; this only covers the case where it is not.
      if (!openCard) commandCard(fileTitle(data), data.why, 'confirm');
      askConfirm(data);
      return;
    }
    if (data.type === 'confirm') {
      // Never answer on the user's behalf: if the card went missing, draw one
      // rather than silently declining a command they never saw. Either way it
      // is already open -- commandCard opens anything that is not automatic --
      // so nothing here resizes a card the thread has already laid out.
      if (!openCard) commandCard(data.command, data.why, 'confirm');
      askConfirm(data);
      return;
    }
    if (data.type === 'memoryUndone') {
      markUndone(data.token, Boolean(data.ok));
      return;
    }
    if (data.type === 'state') {
      setRunning(Boolean(data.running));
      return;
    }
    if (data.type === 'history') {
      paintHistory(data.items || [], data.current);
      return;
    }
    if (data.type === 'memoryList') {
      paintMemory(data);
      return;
    }
    if (data.type === 'memoryError') {
      // Only the stale-view answer is worth a word: the list underneath has just
      // been redrawn from disk, so what the user is looking at is already right,
      // and they need to know why what they typed is not in it.
      const notice = $('memoryNotice');
      notice.textContent = data.outcome === 'full' ? S.memoryFull : S.memoryStale;
      notice.hidden = false;
      return;
    }
    if (data.type === 'skillList') {
      paintSkills(data);
      return;
    }
    if (data.type === 'trustList') {
      paintTrust(data);
      return;
    }
    if (data.type === 'modelList') {
      paintModels(data);
      return;
    }
    if (data.type === 'usage') {
      setUsage(data.usage, data.round);
      return;
    }
    if (data.type === 'restore') {
      replay(data.entries || []);
      // What that conversation cost, not what the one it replaced did. The round
      // is zeroed either way: whatever it was mid-way through is over.
      setUsage(data.usage, { prompt: 0, completion: 0, requests: 0 });
      return;
    }
    if (data.type === 'cleared') {
      clearThread();
      closeConfirm();
      setUsage(null, { prompt: 0, completion: 0, requests: 0 });
      setRunning(false);
    }
  });

  // -- composer -------------------------------------------------------------

  /** Whether a task is under way, which is what the one button is asking about. */
  function isRunning() {
    return $('send').classList.contains('running');
  }

  function send() {
    const input = $('input');
    const text = input.value.trim();
    if (!text) return;
    input.value = '';
    autoGrow();
    setRunning(false);
    post('send', { text: text });
  }

  function submit() {
    if (isRunning()) post('stop');
    else send();
  }

  $('send').onclick = submit;
  $('input').addEventListener('input', autoGrow);
  // Enabled only when there is something to send, which is also what tells the
  // reader that Enter would do nothing yet.
  $('input').addEventListener('input', () => { if (!isRunning()) setRunning(false); });
  $('newChat').onclick = () => post('newChat');
  /** The toolbar's live settings, drawn from whatever the extension last said. */
  function applyAiState(state) {
    if (state.thinking && typeof state.thinking === 'object') applyThinkState(state.thinking);
    if (typeof state.mode === 'string') paintMode(state.mode);
    if (typeof state.model === 'string') {
      const chip = $('model');
      chip.textContent = state.model;
      // The row is narrow and a model name is not, so the tooltip carries the
      // whole of it -- and the label the button already had says what it is for.
      chip.title = (S.agentModelTitle || '') + ': ' + state.model;
      chip.setAttribute('aria-label', chip.title);
    }
  }

  /*
   * Both of these ask rather than decide. The setting is written to the config
   * file and comes back as `aiState`, which is what actually moves the button --
   * so a panel never shows a state the file disagrees with, and every other open
   * panel moves at the same moment.
   */
  $('think').onclick = () => { openPanel('thinkPanel'); paintThinkList(); };
  $('history').onclick = openHistory;
  $('historyClose').onclick = closeHistory;

  $('model').onclick = () => openPanel('modelPanel', 'modelList');
  $('memory').onclick = () => openPanel('memoryPanel', 'memoryList');
  $('skills').onclick = () => openPanel('skillPanel', 'skillList');
  // Drawn from what this side already knows rather than asked for: the mode
  // arrives on every `aiState`, so there is nothing to fetch.
  $('mode').onclick = () => { openPanel('modePanel'); paintModeList(); };
  $('trust').onclick = () => openPanel('trustPanel', 'trustList');
  $('memoryClose').onclick = closePanels;
  $('skillClose').onclick = closePanels;
  $('thinkClose').onclick = closePanels;
  $('modeClose').onclick = closePanels;
  $('trustClose').onclick = closePanels;
  $('trustEdit').onclick = () => post('openTrust');
  $('modelClose').onclick = closePanels;
  $('skillFolder').onclick = () => post('openSkillFolder');
  $('modelAdd').onclick = () => {
    $('modelForm').hidden = false;
    $('modelBaseUrl').focus();
  };
  $('modelSave').onclick = submitModel;
  $('modelCancel').onclick = () => {
    $('modelKey').value = '';
    $('modelForm').hidden = true;
  };
  // Enter anywhere in the form submits it, the way it would in a dialog.
  ['modelBaseUrl', 'modelName', 'modelKey'].forEach((id) => {
    $(id).onkeydown = (event) => {
      if (event.key === 'Enter') { event.preventDefault(); submitModel(); }
    };
  });

  // Clicking away from any of them closes it, which is what a backdrop is for.
  panels.forEach((name) => {
    $(name).onclick = (event) => { if (event.target === $(name)) closePanels(); };
  });

  /*
   * Esc, in the order the things it could mean are stacked on screen.
   *
   * The permission prompt owns it first and answers it itself, so this never sees
   * one. Then the history overlay, which it dismisses like any overlay. With
   * neither of those in the way, what is in front of the user is a running task,
   * and Esc is the fastest thing on the keyboard to reach for when they want it
   * to stop -- from anywhere in the panel, the composer included.
   */
  document.addEventListener('keydown', (event) => {
    if (event.key !== 'Escape' || pending !== null) return;
    if (openPanelId()) {
      closePanels();
      event.preventDefault();
      return;
    }
    if (!isRunning()) return;
    post('stop');
    event.preventDefault();
  });

  $('input').addEventListener('keydown', (event) => {
    // Enter sends; Shift+Enter is how you write a second line.
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  });

  applyStrings();
  $('emptyMark').append(window.tshellMark());
  applyAiState({ thinking: boot.thinking || {}, mode: boot.mode || 'ask', model: boot.model || '' });
  setRunning(false);
  autoGrow();
  post('ready');
}());
