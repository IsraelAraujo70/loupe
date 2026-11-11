#!/usr/bin/env python3
"""Builds the Loupe website into site/public: English at /, Portuguese at /pt/.

    python3 site/build.py

The Dockerfile runs it; public/ is not committed.
Both pages come from one template, so the languages cannot drift apart.
Only the standard library is used.
"""

import html
import shutil
import sys
from pathlib import Path

SITE = Path(__file__).resolve().parent
OUT = SITE / "public"
REPO = "https://github.com/IsraelAraujo70/loupe"
LATEST = f"{REPO}/releases/latest"
MAC = f"{LATEST}/download/loupe-macos-aarch64.zip"
LINUX = f"{LATEST}/download/loupe-linux-x86_64"
URL = "https://loupe.israeldeveloper.com.br"

EN = {
    "lang": "en",
    "path": "/",
    "title": "Loupe: GitHub reviews and Linear issues, in one fast desktop app",
    "description": "Loupe is a free, open-source desktop app for macOS and Linux that puts your GitHub pull requests, Linear issues, and CI runs in one fast, keyboard-first window.",
    "nav_features": "Features",
    "nav_download": "Download",
    "nav_faq": "FAQ",
    "switch_label": "Português",
    "switch_href": "/pt/",
    "badge": "Open source · AGPL",
    "hero": "GitHub reviews and Linear issues, in one fast desktop app.",
    "lede": "Loupe shows the pull requests waiting on you, the issues they close, and the CI behind them in one native window. Tabs, a command palette, and the keyboard first.",
    "cta_mac": "Download for macOS",
    "cta_linux": "Download for Linux",
    "cta_source": "View the source",
    "hero_caption": "Reviews: what waits on you, what you opened, what mentions you, and every other open pull request of the repositories a view follows.",
    "hero_alt": "Loupe's Reviews screen: three sections of pull requests and every other open pull request of a followed repository",
    "trust_title": "Yours from the first launch.",
    "trust": [
        ("Your tokens stay on your machine", "Loupe talks to GitHub and Linear straight from your computer. Tokens live in the macOS Keychain or in your Linux config folder."),
        ("Open source", "AGPL license: read the code, audit it, build it yourself, and contribute."),
        ("macOS and Linux", "A native app built in Rust with GPUI. Apple silicon and x86_64 Linux builds, updated in place."),
        ("No account with us", "There is no Loupe server and nothing to sign up for. Sign in to GitHub and add a Linear key."),
    ],
    "week_title": "What changes in your week.",
    "week_lede": "The four things you do between commits, without a dozen browser tabs.",
    "week": [
        ("Triage the inbox", "GitHub notifications filtered by organization, repository, or kind, and cleared in bulk."),
        ("Review a pull request", "Conversation, files with a highlighted diff, and checks in one tab. Comment, approve, or request changes."),
        ("Move a ticket", "Your Linear board next to the pull request that closes it. Change the status, assignee, or labels in place."),
        ("See the CI", "Every check of a pull request, the workflow run behind it as a graph, the log of the step that failed, and Re-run."),
    ],
    "features_title": "Everything a review needs, a keystroke away.",
    "features": [
        ("pull-request", "Pull requests", "Files with a syntax-highlighted diff, unified or split, with a tree you can filter and mark as viewed. Review threads, suggestions, and the merge box live in the same tab.", "A pull request's Files tab with the file tree and a unified diff"),
        ("checks", "Checks", "Every check of the head commit, which ones are required, how long each took, and a link to the job.", "A pull request's Checks tab listing nineteen passing checks"),
        ("actions", "Actions", "A workflow run as the graph of its jobs and matrices, with its annotations, artifacts, and Re-run.", "A workflow run drawn as a graph of jobs and matrix groups"),
        ("linear-board", "Linear", "Team boards, your issues, projects, and saved views, across several workspaces. Each pull request shows the Linear issues it closes.", "A Linear team board with In Progress, In Review, Todo, and Backlog columns"),
        ("palette", "Command palette", "Cmd+K jumps to any pull request, repository, issue, or screen. Narrow it with pr:, repo:, org:, or issue:.", "The command palette listing places to go"),
        ("themes", "Themes", "Six themes, each with a light and a dark palette, switched live, plus contrast and density.", "The Appearance settings with six themes"),
    ],
    "t3_title": "Review with T3 Code",
    "t3_body": "Connect a T3 Code server and hand a pull request to it: Loupe writes the review request, picks the project that tracks the repository, and the server starts the review thread in a worktree of the pull request's head.",
    "t3_request_label": "The review request Loupe writes",
    "t3_steps": [
        "Read the repository's AGENTS.md, the description, and the reviews so far.",
        "Check that it solves what it claims, against the linked Linear issue.",
        "List findings with file:line, severity, and a proof.",
        "Run the tests it touches and show a mutation that fails one.",
        "Say what leaked out of scope. Never comment, approve, or push.",
    ],
    "download_title": "Download Loupe.",
    "download_lede": "Free, for macOS and Linux. Loupe checks for a new release every hour and updates itself.",
    "mac_title": "macOS",
    "mac_note": "Apple silicon. Signed, not notarized: the first time, right-click Loupe.app and choose Open.",
    "linux_title": "Linux",
    "linux_note": "x86_64, X11 or Wayland, with a Vulkan driver. Run chmod +x on the file, then start it.",
    "source_title": "From source",
    "source_note": "Rust 1.99.1. Clone the repository and run cargo run --release.",
    "download_mac": "Download for macOS",
    "download_linux": "Download for Linux",
    "download_source": "Build instructions",
    "faq_title": "Questions.",
    "faq": [
        ("Is Loupe free?", "Yes. Loupe is free software under the GNU AGPL v3.0. There is no paid plan."),
        ("Where do my tokens go?", "Only to the API they belong to: GitHub tokens to GitHub, Linear keys to Linear, the T3 token to your T3 server. Loupe has no server of its own and collects no analytics."),
        ("Which GitHub permissions does it need?", "Sign in with the device flow (scopes repo, read:org, and notifications), paste a personal access token, or let Loupe use the token of the GitHub CLI."),
        ("Do I need Linear?", "No. Without a Linear key you still get Reviews, the Inbox, pull requests, and Actions. Linear adds boards, issues, and the issues each pull request closes."),
        ("Windows? Intel Macs?", "Not yet. Releases are built for Apple silicon and x86_64 Linux. You can build from source on other platforms GPUI supports."),
        ("What is T3 Code?", "An open-source server that runs coding agents in threads. Loupe can start a review thread on it for a pull request; everything else works without it."),
    ],
    "footer_license": "AGPL-3.0 license",
    "footer_made": "Built with Rust and GPUI.",
}

PT = {
    "lang": "pt-BR",
    "path": "/pt/",
    "title": "Loupe: revisões do GitHub e issues do Linear num app de desktop rápido",
    "description": "Loupe é um app de desktop gratuito e open source para macOS e Linux que junta pull requests do GitHub, issues do Linear e o CI numa janela rápida, feita para o teclado.",
    "nav_features": "Recursos",
    "nav_download": "Baixar",
    "nav_faq": "Perguntas",
    "switch_label": "English",
    "switch_href": "/",
    "badge": "Open source · AGPL",
    "hero": "Revisões do GitHub e issues do Linear, num app de desktop rápido.",
    "lede": "O Loupe mostra os pull requests que esperam por você, as issues que eles fecham e o CI por trás, numa janela nativa. Abas, paleta de comandos e teclado em primeiro lugar.",
    "cta_mac": "Baixar para macOS",
    "cta_linux": "Baixar para Linux",
    "cta_source": "Ver o código",
    "hero_caption": "Reviews: o que espera por você, o que você abriu, onde te mencionaram, e todos os outros PRs abertos dos repositórios que uma view acompanha.",
    "hero_alt": "Tela Reviews do Loupe: três seções de pull requests e todos os outros pull requests abertos de um repositório acompanhado",
    "trust_title": "Seu desde a primeira abertura.",
    "trust": [
        ("Seus tokens ficam na sua máquina", "O Loupe fala com o GitHub e o Linear direto do seu computador. Os tokens ficam no Keychain do macOS ou na pasta de configuração do Linux."),
        ("Open source", "Licença AGPL: leia o código, audite, compile você mesmo e contribua."),
        ("macOS e Linux", "App nativo em Rust com GPUI. Builds para Apple silicon e Linux x86_64, que se atualizam sozinhos."),
        ("Sem conta com a gente", "Não existe servidor do Loupe nem cadastro. Entre no GitHub e adicione uma chave do Linear."),
    ],
    "week_title": "O que muda na sua semana.",
    "week_lede": "As quatro coisas que você faz entre um commit e outro, sem uma dúzia de abas no navegador.",
    "week": [
        ("Triar o inbox", "Notificações do GitHub filtradas por organização, repositório ou tipo, e marcadas como lidas em lote."),
        ("Revisar um PR", "Conversa, arquivos com diff colorido e checks na mesma aba. Comente, aprove ou peça mudanças."),
        ("Mover um ticket", "O board do Linear ao lado do PR que fecha a issue. Mude status, responsável ou labels ali mesmo."),
        ("Ver o CI", "Todos os checks do PR, o workflow por trás como um grafo, o log do passo que falhou e o Re-run."),
    ],
    "features_title": "Tudo o que uma revisão pede, a um atalho de distância.",
    "features": [
        ("pull-request", "Pull requests", "Arquivos com diff colorido, unificado ou lado a lado, com uma árvore que você filtra e marca como vista. Threads de revisão, sugestões e o merge ficam na mesma aba.", "A aba Files de um PR com a árvore de arquivos e um diff unificado"),
        ("checks", "Checks", "Todos os checks do último commit, quais são obrigatórios, quanto cada um levou e o link para o job.", "A aba Checks de um PR com dezenove checks passando"),
        ("actions", "Actions", "Um workflow como o grafo dos seus jobs e matrizes, com anotações, artefatos e Re-run.", "Um workflow desenhado como grafo de jobs e grupos de matriz"),
        ("linear-board", "Linear", "Boards dos times, suas issues, projetos e views salvas, em vários workspaces. Cada PR mostra as issues do Linear que ele fecha.", "Um board do Linear com as colunas In Progress, In Review, Todo e Backlog"),
        ("palette", "Paleta de comandos", "Cmd+K leva a qualquer PR, repositório, issue ou tela. Refine com pr:, repo:, org: ou issue:.", "A paleta de comandos listando destinos"),
        ("themes", "Temas", "Seis temas, cada um com paleta clara e escura, trocados na hora, além de contraste e densidade.", "As configurações de aparência com seis temas"),
    ],
    "t3_title": "Revisar com o T3 Code",
    "t3_body": "Conecte um servidor T3 Code e entregue um PR a ele: o Loupe escreve o pedido de revisão, escolhe o projeto que acompanha o repositório, e o servidor abre a thread de revisão numa worktree do head do PR.",
    "t3_request_label": "O pedido de revisão que o Loupe escreve",
    "t3_steps": [
        "Ler o AGENTS.md do repositório, a descrição e as revisões já feitas.",
        "Conferir se resolve o que diz resolver, contra a issue ligada no Linear.",
        "Listar achados com arquivo:linha, gravidade e uma prova.",
        "Rodar os testes que o PR toca e mostrar uma mutação que derruba um.",
        "Dizer o que vazou do escopo. Nunca comentar, aprovar nem fazer push.",
    ],
    "download_title": "Baixe o Loupe.",
    "download_lede": "Gratuito, para macOS e Linux. O Loupe procura uma versão nova a cada hora e se atualiza sozinho.",
    "mac_title": "macOS",
    "mac_note": "Apple silicon. Assinado, sem notarização: na primeira vez, clique com o botão direito no Loupe.app e escolha Abrir.",
    "linux_title": "Linux",
    "linux_note": "x86_64, X11 ou Wayland, com driver Vulkan. Rode chmod +x no arquivo e abra.",
    "source_title": "Pelo código",
    "source_note": "Rust 1.99.1. Clone o repositório e rode cargo run --release.",
    "download_mac": "Baixar para macOS",
    "download_linux": "Baixar para Linux",
    "download_source": "Como compilar",
    "faq_title": "Perguntas.",
    "faq": [
        ("O Loupe é gratuito?", "Sim. O Loupe é software livre sob a GNU AGPL v3.0. Não existe plano pago."),
        ("Para onde vão os meus tokens?", "Só para a API de cada um: token do GitHub para o GitHub, chave do Linear para o Linear, token do T3 para o seu servidor T3. O Loupe não tem servidor próprio nem coleta métricas."),
        ("Quais permissões do GitHub ele pede?", "Entre pelo device flow (escopos repo, read:org e notifications), cole um personal access token, ou deixe o Loupe usar o token do GitHub CLI."),
        ("Preciso do Linear?", "Não. Sem chave do Linear você tem Reviews, Inbox, pull requests e Actions. O Linear acrescenta boards, issues e as issues que cada PR fecha."),
        ("Windows? Mac Intel?", "Ainda não. Os releases saem para Apple silicon e Linux x86_64. Dá para compilar pelo código em outras plataformas que o GPUI suporta."),
        ("O que é o T3 Code?", "Um servidor open source que roda agentes de código em threads. O Loupe pode abrir nele uma thread de revisão para um PR; o resto funciona sem ele."),
    ],
    "footer_license": "Licença AGPL-3.0",
    "footer_made": "Feito com Rust e GPUI.",
}

ICONS = {
    "lock": '<path d="M7 11V7a5 5 0 0 1 10 0v4"/><rect x="4" y="11" width="16" height="10" rx="2"/>',
    "code": '<path d="m16 18 6-6-6-6"/><path d="m8 6-6 6 6 6"/>',
    "monitor": '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8"/><path d="M12 17v4"/>',
    "user": '<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/><path d="m3 3 18 18"/>',
    "inbox": '<path d="M22 12h-6l-2 3h-4l-2-3H2"/><path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z"/>',
    "pr": '<circle cx="6" cy="6" r="3"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="18" r="3"/><path d="M6 9v6"/><path d="M13 6h3a2 2 0 0 1 2 2v7"/>',
    "board": '<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M9 3v18"/><path d="M15 3v18"/>',
    "check": '<circle cx="12" cy="12" r="10"/><path d="m9 12 2 2 4-4"/>',
    "github": '<path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.4 5.4 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65S8.93 17.38 9 18v4"/><path d="M9 18c-4.51 2-5-2-7-2"/>',
    "download": '<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>',
}
TRUST_ICONS = ["lock", "code", "monitor", "user"]
WEEK_ICONS = ["inbox", "pr", "board", "check"]


def icon(name: str) -> str:
    return (
        '<svg class="icon" viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" '
        f'stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{ICONS[name]}</svg>'
    )


def e(text: str) -> str:
    return html.escape(text, quote=True)


def shot(name: str, alt: str, eager: bool = False) -> str:
    loading = 'fetchpriority="high"' if eager else 'loading="lazy" decoding="async"'
    return (
        f'<img src="/img/{name}.webp" srcset="/img/{name}-800.webp 800w, /img/{name}.webp 1440w" '
        f'sizes="(max-width: 1100px) 100vw, 1100px" width="1440" height="900" alt="{e(alt)}" {loading}>'
    )


def page(t: dict) -> str:
    other = PT if t is EN else EN
    trust = "\n".join(
        f'<li>{icon(i)}<div><h3>{e(a)}</h3><p>{e(b)}</p></div></li>' for i, (a, b) in zip(TRUST_ICONS, t["trust"])
    )
    week = "\n".join(
        f'<li><span class="chip">{icon(i)}</span><h3>{e(a)}</h3><p>{e(b)}</p></li>' for i, (a, b) in zip(WEEK_ICONS, t["week"])
    )
    features = "\n".join(
        f'<article class="feature"><div class="copy"><h3>{e(title)}</h3><p>{e(body)}</p></div>'
        f'<figure class="frame">{shot(name, alt)}</figure></article>'
        for name, title, body, alt in t["features"]
    )
    steps = "".join(f"<li>{e(step)}</li>" for step in t["t3_steps"])
    faq = "\n".join(f"<details><summary>{e(q)}</summary><p>{e(a)}</p></details>" for q, a in t["faq"])
    return f"""<!doctype html>
<html lang="{t['lang']}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{e(t['title'])}</title>
<meta name="description" content="{e(t['description'])}">
<meta name="color-scheme" content="dark">
<meta name="theme-color" content="#0e0f11">
<link rel="canonical" href="{URL}{t['path']}">
<link rel="alternate" hreflang="en" href="{URL}/">
<link rel="alternate" hreflang="pt-BR" href="{URL}/pt/">
<link rel="alternate" hreflang="x-default" href="{URL}/">
<link rel="icon" href="/icon.svg" type="image/svg+xml">
<meta property="og:type" content="website">
<meta property="og:title" content="{e(t['title'])}">
<meta property="og:description" content="{e(t['description'])}">
<meta property="og:url" content="{URL}{t['path']}">
<meta property="og:image" content="{URL}/img/reviews.webp">
<meta name="twitter:card" content="summary_large_image">
<link rel="stylesheet" href="/style.css">
</head>
<body>
<a class="skip" href="#main">{'Skip to content' if t is EN else 'Pular para o conteúdo'}</a>
<header class="top">
  <div class="wrap bar">
    <a class="brand" href="{t['path']}"><img src="/icon.svg" width="28" height="28" alt=""> Loupe</a>
    <nav aria-label="{'Main' if t is EN else 'Principal'}">
      <a href="#features">{e(t['nav_features'])}</a>
      <a href="#download">{e(t['nav_download'])}</a>
      <a href="#faq">{e(t['nav_faq'])}</a>
      <a href="{t['switch_href']}" hreflang="{other['lang']}" lang="{other['lang']}">{e(t['switch_label'])}</a>
      <a class="gh" href="{REPO}" aria-label="GitHub">{icon('github')}</a>
    </nav>
  </div>
</header>
<main id="main">
<section class="hero wrap">
  <p class="badge"><span class="dot"></span>{e(t['badge'])}</p>
  <h1>{e(t['hero'])}</h1>
  <p class="lede">{e(t['lede'])}</p>
  <div class="ctas">
    <a class="btn primary" href="{MAC}">{icon('download')}{e(t['cta_mac'])}</a>
    <a class="btn" href="{LINUX}">{icon('download')}{e(t['cta_linux'])}</a>
    <a class="btn ghost" href="{REPO}">{icon('github')}{e(t['cta_source'])}</a>
  </div>
  <figure class="hero-shot"><div class="frame">{shot('reviews', t['hero_alt'], eager=True)}</div><figcaption>{e(t['hero_caption'])}</figcaption></figure>
</section>

<section class="wrap block" aria-labelledby="trust">
  <h2 id="trust">{e(t['trust_title'])}</h2>
  <ul class="trust">
{trust}
  </ul>
</section>

<section class="wrap block" aria-labelledby="week">
  <h2 id="week">{e(t['week_title'])}</h2>
  <p class="sub">{e(t['week_lede'])}</p>
  <ul class="week">
{week}
  </ul>
</section>

<section class="wrap block" id="features" aria-labelledby="features-title">
  <h2 id="features-title">{e(t['features_title'])}</h2>
  <div class="features">
{features}
    <article class="feature t3"><div class="copy"><h3>{e(t['t3_title'])}</h3><p>{e(t['t3_body'])}</p></div>
    <div class="request" role="group" aria-label="{e(t['t3_request_label'])}"><p class="request-head">{icon('pr')}{e(t['t3_request_label'])}</p><ol>{steps}</ol></div></article>
  </div>
</section>

<section class="wrap block" id="download" aria-labelledby="download-title">
  <h2 id="download-title">{e(t['download_title'])}</h2>
  <p class="sub">{e(t['download_lede'])}</p>
  <div class="cards">
    <div class="card"><h3>{e(t['mac_title'])}</h3><p>{e(t['mac_note'])}</p><a class="btn primary" href="{MAC}">{icon('download')}{e(t['download_mac'])}</a></div>
    <div class="card"><h3>{e(t['linux_title'])}</h3><p>{e(t['linux_note'])}</p><a class="btn primary" href="{LINUX}">{icon('download')}{e(t['download_linux'])}</a></div>
    <div class="card"><h3>{e(t['source_title'])}</h3><p>{e(t['source_note'])}</p><a class="btn" href="{REPO}#build-from-source">{icon('code')}{e(t['download_source'])}</a></div>
  </div>
</section>

<section class="wrap block" id="faq" aria-labelledby="faq-title">
  <h2 id="faq-title">{e(t['faq_title'])}</h2>
  <div class="faq">
{faq}
  </div>
</section>
</main>
<footer class="wrap foot">
  <a class="brand" href="{t['path']}"><img src="/icon.svg" width="22" height="22" alt=""> Loupe</a>
  <p>{e(t['footer_made'])} <a href="{REPO}/blob/main/LICENSE">{e(t['footer_license'])}</a> · <a href="{REPO}">GitHub</a> · <a href="{t['switch_href']}" lang="{other['lang']}">{e(t['switch_label'])}</a></p>
</footer>
</body>
</html>
"""


def outputs() -> dict[Path, bytes]:
    files = {
        OUT / "index.html": page(EN).encode(),
        OUT / "pt" / "index.html": page(PT).encode(),
        OUT / "style.css": (SITE / "style.css").read_bytes(),
        OUT / "icon.svg": (SITE / "icon.svg").read_bytes(),
        OUT / "404.html": (SITE / "404.html").read_bytes(),
    }
    for image in sorted((SITE / "img").glob("*.webp")):
        files[OUT / "img" / image.name] = image.read_bytes()
    return files


def main() -> int:
    files = outputs()
    shutil.rmtree(OUT, ignore_errors=True)
    for path, data in files.items():
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    print(f"wrote {len(files)} files to {OUT.relative_to(SITE.parent)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
