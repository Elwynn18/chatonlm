// chatonlm.com : animations et comportements des pages. Tout fonctionne sans ce
// script (liens de téléchargement, formulaire, lecture) ; il ajoute le fil qui
// se déroule, le regard du chaton, la fenêtre de démonstration, l'état de la
// version publiée et l'envoi du formulaire sans quitter la page.

const html = document.documentElement;
const calme = matchMedia("(prefers-reduced-motion: reduce)").matches;
html.classList.remove("sans-js");

const borne = (n, min, max) => Math.max(min, Math.min(max, n));
const vu = (el, action, seuil = 0.4) => {
  const observateur = new IntersectionObserver(
    (entrees) => {
      if (entrees.some((e) => e.isIntersecting)) {
        observateur.disconnect();
        action();
      }
    },
    { threshold: seuil },
  );
  observateur.observe(el);
};

// --- En-tête : fond dès que la page a défilé --------------------------------
function entete() {
  const barre = document.querySelector("[data-entete]");
  if (!barre) return;
  const maj = () => barre.classList.toggle("entete--colle", scrollY > 8);
  addEventListener("scroll", maj, { passive: true });
  maj();
}

// --- Chaton : les pupilles suivent le pointeur, sinon il regarde ailleurs ---
function chatons() {
  const chats = [...document.querySelectorAll("svg[data-chaton]")].map(
    (svg) => ({
      svg,
      pupilles: svg.querySelector(".c-pupils"),
    }),
  );
  if (!chats.length) return;
  const regarde = (x, y) => {
    for (const { svg, pupilles } of chats) {
      const r = svg.getBoundingClientRect();
      if (!r.width || r.bottom < 0 || r.top > innerHeight) continue;
      const dx = x - (r.left + r.width / 2);
      const dy = y - (r.top + r.height * 0.45);
      const d = Math.hypot(dx, dy) || 1;
      const k = Math.min(1, d / 260);
      pupilles.setAttribute(
        "transform",
        `translate(${((dx / d) * k * 9).toFixed(1)} ${((dy / d) * k * 8).toFixed(1)})`,
      );
    }
  };
  let dernier = 0;
  addEventListener(
    "pointermove",
    (e) => {
      dernier = performance.now();
      regarde(e.clientX, e.clientY);
    },
    { passive: true },
  );
  if (calme) return;
  // Sans pointeur depuis un moment (ou sur un écran tactile) : un coup d'œil au hasard.
  setInterval(() => {
    if (performance.now() - dernier > 3500)
      regarde(Math.random() * innerWidth, Math.random() * innerHeight);
  }, 2600);
  // De temps en temps, une oreille frémit.
  const oreilles = [...document.querySelectorAll(".c-ear")];
  setInterval(() => {
    const oreille = oreilles[Math.floor(Math.random() * oreilles.length)];
    oreille?.animate(
      [
        { transform: "rotate(0)" },
        { transform: "rotate(-11deg)" },
        { transform: "rotate(5deg)" },
        { transform: "rotate(0)" },
      ],
      { duration: 420, easing: "ease-out" },
    );
  }, 4200);
}

// --- Le fil ------------------------------------------------------------------
// Une seule ligne passe par les repères [data-fil] de la page, dans l'ordre du
// document, et se dessine à mesure que l'on descend. Un repère « perle » pose
// une perle et révèle sa carte ; le repère « maison » fait dessiner au fil le
// contour d'une maison autour de son cadre.

// Temps que met le fil à tracer la maison, en millisecondes.
const DUREE_MAISON = 3400;
const SVG = "http://www.w3.org/2000/svg";
const aLEcran = (el) => {
  const r = el.getBoundingClientRect();
  return r.bottom > 0 && r.top < innerHeight;
};
// Point d'un segment du fil : droite [a, b] ou courbe de Bézier [a, c1, c2, b].
const sur = (s, t) => {
  if (s.length === 2)
    return [s[0].x + (s[1].x - s[0].x) * t, s[0].y + (s[1].y - s[0].y) * t];
  const u = 1 - t;
  const [k0, k1, k2, k3] = [u * u * u, 3 * u * u * t, 3 * u * t * t, t * t * t];
  return [
    k0 * s[0].x + k1 * s[1].x + k2 * s[2].x + k3 * s[3].x,
    k0 * s[0].y + k1 * s[1].y + k2 * s[2].y + k3 * s[3].y,
  ];
};

function fil() {
  const toile = document.querySelector("[data-fil-toile]");
  if (!toile) return;
  const page = toile.parentElement;
  const trait = toile.querySelector(".fil-trait");
  const perles = toile.querySelector(".fil-perles");
  const pelote = document.querySelector("[data-pelote]");
  let longueur = 0;
  let longueurs = new Float32Array(0);
  let hauteurs = new Float32Array(0);
  let marques = [];
  let maison = null;
  let largeur = 0;
  let courant = 0;
  let cible = 0;
  let image = 0;
  let instant = 0;
  let parti = false;

  function tracer() {
    const origine = page.getBoundingClientRect();
    const reperes = [...page.querySelectorAll("[data-fil]")].filter(
      (el) => el.getClientRects().length,
    );
    if (reperes.length < 2) {
      html.classList.add("sans-fil");
      return false;
    }
    html.classList.remove("sans-fil");
    const segments = [];
    const points = [];
    let depart = null;
    let avant = null;
    let sens = 1;
    let murs = null;
    const droite = (b) => {
      segments.push([avant, b]);
      avant = b;
    };
    const lier = (b) => {
      const a = avant;
      avant = b;
      if (!a) {
        depart = b;
        return;
      }
      const k = Math.max(56, Math.abs(b.y - a.y) * 0.5);
      // Deux repères à l'aplomb l'un de l'autre : le fil ondule au lieu de tomber droit.
      const bosse = Math.abs(b.x - a.x) < 48 ? 34 * (sens = -sens) : 0;
      segments.push([
        a,
        { x: a.x + bosse, y: a.y + k },
        { x: b.x + bosse, y: b.y - k },
        b,
      ]);
    };
    for (const el of reperes) {
      const r = el.getBoundingClientRect();
      if (el.dataset.fil === "maison") {
        const coin = (u, v) => ({
          x: Number((r.left - origine.left + (u / 100) * r.width).toFixed(1)),
          y: Number((r.top - origine.top + (v / 100) * r.height).toFixed(1)),
        });
        lier(coin(6, 44));
        murs = { el, debut: segments.length };
        // Mur de gauche, sol, mur de droite, puis le toit.
        for (const [u, v] of [
          [6, 96],
          [94, 96],
          [94, 44],
          [50, 6],
          [6, 44],
        ])
          droite(coin(u, v));
        murs.fin = segments.length;
        // Le fil redescend le long du mur et repart du sol.
        droite(coin(6, 96));
        continue;
      }
      // Une carte encore décalée par son entrée ne décale pas sa perle : le
      // repère est pris à la place où la carte se pose.
      const carte = el.closest("[data-fil-carte]");
      const glisse = carte
        ? new DOMMatrixReadOnly(getComputedStyle(carte).transform).m42
        : 0;
      lier({
        x: Math.round(r.left - origine.left + r.width / 2),
        y: Math.round(r.top - glisse - origine.top + r.height / 2),
      });
      if (el.dataset.fil === "perle")
        points.push({ el, p: avant, segment: segments.length });
    }
    // Le bout du fil : une petite boucle.
    const bout = avant;
    const boucle = { x: bout.x - 38, y: bout.y + 12 };
    segments.push(
      [
        bout,
        { x: bout.x - 4, y: bout.y + 30 },
        { x: bout.x - 34, y: bout.y + 34 },
        boucle,
      ],
      [
        boucle,
        { x: boucle.x - 4, y: boucle.y - 22 },
        { x: boucle.x + 26, y: boucle.y - 26 },
        { x: boucle.x + 34, y: boucle.y + 2 },
      ],
    );

    // Le tracé est écrit et mesuré du même geste : chaque segment est découpé en
    // cordes de quelques pixels, dont on garde la longueur parcourue et le point
    // le plus bas atteint (croissant, donc cherchable). Demander ces points au
    // navigateur (getPointAtLength, des milliers de fois) figeait la page une
    // demi-seconde à chaque changement de hauteur.
    let d = `M${depart.x} ${depart.y}`;
    const parcouru = [0];
    const atteint = [depart.y];
    const fins = [];
    let total = 0;
    let bas = depart.y;
    for (const s of segments) {
      const b = s.at(-1);
      d +=
        s.length === 4
          ? `C${s[1].x} ${s[1].y} ${s[2].x} ${s[2].y} ${b.x} ${b.y}`
          : `L${b.x} ${b.y}`;
      let enveloppe = 0;
      for (let i = 1; i < s.length; i += 1)
        enveloppe += Math.hypot(s[i].x - s[i - 1].x, s[i].y - s[i - 1].y);
      const n = borne(Math.ceil(enveloppe / 5), 1, 400);
      let [x0, y0] = [s[0].x, s[0].y];
      for (let i = 1; i <= n; i += 1) {
        const [x, y] = sur(s, i / n);
        total += Math.hypot(x - x0, y - y0);
        bas = Math.max(bas, y);
        parcouru.push(total);
        atteint.push(bas);
        [x0, y0] = [x, y];
      }
      fins.push(total);
    }
    trait.setAttribute("d", d);
    // Les tirets suivent la longueur du navigateur : la nôtre s'y ramène.
    longueur = trait.getTotalLength();
    trait.style.strokeDasharray = `${longueur}`;
    const echelle = total ? longueur / total : 1;
    longueurs = Float32Array.from(parcouru, (l) => l * echelle);
    hauteurs = Float32Array.from(atteint);
    // Longueur de fil déroulée au bout du segment de ce rang.
    const apres = (rang) => (rang ? fins[rang - 1] * echelle : 0);
    maison = murs && {
      el: murs.el,
      debut: apres(murs.debut),
      fin: apres(murs.fin),
    };
    // Les perles déjà posées sont gardées : recréées, elles rejoueraient leur entrée.
    marques = points.map(({ el, p, segment }, i) => {
      const perle =
        perles.children[i] ??
        perles.appendChild(document.createElementNS(SVG, "circle"));
      perle.setAttribute("cx", p.x);
      perle.setAttribute("cy", p.y);
      perle.setAttribute("r", 11);
      return {
        seuil: apres(segment),
        perle,
        carte: el.closest("[data-fil-carte]"),
      };
    });
    while (perles.children.length > points.length) perles.lastChild.remove();
    largeur = page.clientWidth;
    return true;
  }

  function longueurA(y) {
    let debut = 0;
    let fin = hauteurs.length - 1;
    while (debut < fin) {
      const milieu = (debut + fin + 1) >> 1;
      if (hauteurs[milieu] <= y) debut = milieu;
      else fin = milieu - 1;
    }
    return longueurs[debut] ?? 0;
  }

  function peindre() {
    trait.style.strokeDashoffset = `${longueur - courant}`;
    if (pelote)
      pelote.style.transform = `rotate(${(-courant / 5).toFixed(1)}deg)`;
    for (const { seuil, perle, carte } of marques) {
      const atteint = courant >= seuil - 2;
      perle.classList.toggle("vu", atteint);
      carte?.classList.toggle("vu", atteint);
    }
  }

  function avancer(t) {
    const dt = instant ? Math.min(t - instant, 50) : 16;
    instant = t;
    const reste = cible - courant;
    let pas = Math.abs(reste) < 0.6 ? reste : reste * 0.13;
    // La maison se trace à l'allure d'un crayon, pas d'un seul coup : le fil
    // s'arrête à sa porte, puis y avance d'un pas borné, tant qu'elle est à
    // l'écran. Il se rembobine trois fois plus vite.
    if (maison && pas && aLEcran(maison.el)) {
      const { debut, fin } = maison;
      const allure = ((fin - debut) * dt) / DUREE_MAISON;
      if (pas > 0) {
        if (courant < debut) pas = Math.min(pas, debut - courant);
        else if (courant < fin) pas = Math.min(pas, allure);
      } else if (courant > fin) pas = Math.max(pas, fin - courant);
      else if (courant > debut) pas = Math.max(pas, -3 * allure);
    }
    courant = pas === reste ? cible : courant + pas;
    peindre();
    if (courant === cible) {
      image = 0;
      instant = 0;
    } else image = requestAnimationFrame(avancer);
  }

  function viser() {
    if (!parti) return;
    cible = calme
      ? longueur
      : longueurA(innerHeight * 0.74 - page.getBoundingClientRect().top);
    if (calme) courant = cible;
    if (!image) image = requestAnimationFrame(avancer);
  }

  // La page change de taille (fenêtre, polices arrivées, réponse qui se
  // déroule) : le fil est retracé dans l'image même, il suit la mise en page.
  const refaire = () => {
    const part = longueur ? courant / longueur : 0;
    const memeLargeur = page.clientWidth === largeur;
    if (!tracer()) return;
    // À largeur égale, c'est la suite du trajet qui a bougé : le fil déjà
    // déroulé garde sa longueur. Sinon, il garde sa part du trajet.
    courant = memeLargeur ? Math.min(courant, longueur) : part * longueur;
    peindre();
    viser();
  };
  if (tracer()) peindre();
  addEventListener("scroll", viser, { passive: true });
  document.fonts?.ready.then(refaire);
  // La pelote roule d'abord en place ; le fil part ensuite.
  setTimeout(
    () => {
      parti = true;
      viser();
    },
    calme ? 0 : 1100,
  );
  new ResizeObserver(refaire).observe(page);
}

// --- Maison sans le fil (petit écran) : son contour se trace quand elle paraît --
function maisonFixe() {
  const trace = document.querySelector(".maison-fixe path");
  if (!trace || calme) return;
  const cadre = trace.ownerSVGElement;
  cadre.classList.add("maison--attend");
  vu(
    cadre,
    () => {
      cadre.classList.remove("maison--attend");
      // Le dessin est étiré sur son cadre et son trait garde son épaisseur : ses
      // tirets se comptent en pixels de l'écran. Le plus long des deux tracés
      // est le toit suivi du mur de droite.
      const { width, height } = cadre.getBoundingClientRect();
      const long =
        2 * Math.hypot(0.44 * width, 0.38 * height) + 0.52 * height + 4;
      trace.animate(
        [
          { strokeDasharray: `${long}px`, strokeDashoffset: `${long}px` },
          { strokeDasharray: `${long}px`, strokeDashoffset: "0px" },
        ],
        { duration: 2600, easing: "ease-in-out" },
      );
    },
    0.35,
  );
}

// --- Fenêtre de démonstration --------------------------------------------------
// Un clic montre un outil ; sans clic, la fenêtre les fait défiler, tant qu'elle
// est à l'écran et que le lecteur ne s'y attarde pas (survol, focus). La fenêtre
// dessinée se clique aussi ([data-vers] : son menu, ses conversations, les
// boutons de sa zone de saisie) : même effet que les boutons des outils, qui
// restent les seuls commandes annoncées (la scène est décorative, aria-hidden).
function demo() {
  const racine = document.querySelector("[data-demo]");
  if (!racine) return;
  const app = racine.querySelector(".app");
  const groupe = racine.querySelector(".onglets");
  const textes = racine.querySelector("[data-outil-textes]");
  const onglets = [...racine.querySelectorAll(".onglet")];
  const reponse = racine.querySelector(".reponse")?.firstChild;
  const texte = reponse?.textContent ?? "";
  let index = 0;
  let auto = !calme;
  let visible = false;
  let occupe = false;
  let dejaVue = false;
  let minuteur = 0;
  let frappe = 0;

  const ecrire = () => {
    if (calme || !reponse) return;
    clearInterval(frappe);
    let n = 0;
    reponse.textContent = "";
    frappe = setInterval(() => {
      n += 2;
      reponse.textContent = texte.slice(0, n);
      if (n >= texte.length) clearInterval(frappe);
    }, 26);
  };
  const montrer = (i) => {
    index = i;
    const vue = onglets[i].dataset.vue;
    app.dataset.app = vue;
    racine.dataset.outil = vue;
    onglets.forEach((onglet, k) =>
      onglet.setAttribute("aria-pressed", String(k === i)),
    );
    // Sur un petit écran, les outils sont sur une ligne qui défile : garder le bon en vue.
    if (groupe.scrollWidth > groupe.clientWidth) {
      groupe.scrollTo({
        left: onglets[i].offsetLeft - 16,
        behavior: calme ? "auto" : "smooth",
      });
    }
    if (vue === "chat") ecrire();
  };
  const tourner = () => {
    clearTimeout(minuteur);
    const actif = auto && visible && !occupe;
    groupe.classList.toggle("onglets--auto", actif);
    if (!actif) return;
    minuteur = setTimeout(() => {
      montrer((index + 1) % onglets.length);
      tourner();
    }, 5000);
  };
  const choisir = (i) => {
    auto = false;
    // Le lecteur choisit désormais : le texte de l'outil lui est annoncé.
    textes?.setAttribute("aria-live", "polite");
    tourner();
    montrer(i);
  };
  onglets.forEach((onglet, i) =>
    onglet.addEventListener("click", () => choisir(i)),
  );
  app.addEventListener("click", (e) => {
    const vers = e.target.closest("[data-vers]")?.dataset.vers;
    const i = onglets.findIndex((onglet) => onglet.dataset.vue === vers);
    // Sur l'outil déjà montré, le clic arrête au moins le défilement automatique.
    if (i !== -1 && (i !== index || auto)) choisir(i);
  });
  const attarde = (oui) => () => {
    occupe = oui;
    tourner();
  };
  racine.addEventListener("pointerenter", attarde(true));
  racine.addEventListener("pointerleave", attarde(false));
  racine.addEventListener("focusin", attarde(true));
  racine.addEventListener("focusout", attarde(false));
  new IntersectionObserver(
    ([entree]) => {
      visible = entree.isIntersecting;
      if (visible && !dejaVue) {
        dejaVue = true;
        ecrire();
      }
      tourner();
    },
    { threshold: 0.25 },
  ).observe(app);
}

// --- Questions : la réponse se déroule au lieu de paraître d'un coup -------------
// Sans script ou en mouvement réduit, <details> s'ouvre comme le navigateur le fait.
function questions() {
  if (calme) return;
  for (const bloc of document.querySelectorAll(".questions details")) {
    const titre = bloc.querySelector("summary");
    let ouvert = bloc.open;
    let trajet = null;
    titre.addEventListener("click", (e) => {
      e.preventDefault();
      ouvert = !ouvert;
      const de = bloc.offsetHeight;
      trajet?.cancel();
      // Fermé, <details> ne mesure pas sa réponse : il reste ouvert le temps des deux trajets.
      bloc.open = true;
      const vers = ouvert ? bloc.offsetHeight : titre.offsetHeight;
      bloc.classList.add("deroule");
      bloc.classList.toggle("deroule--ferme", !ouvert);
      const cours = bloc.animate(
        { height: [`${de}px`, `${vers}px`] },
        { duration: 360, easing: "cubic-bezier(0.22, 0.61, 0.36, 1)" },
      );
      trajet = cours;
      cours.onfinish = () => {
        if (trajet !== cours) return;
        trajet = null;
        if (!ouvert) bloc.open = false;
        bloc.classList.remove("deroule", "deroule--ferme");
      };
    });
    // Ouvert ou fermé d'ailleurs (recherche dans la page) : l'état suit, et un
    // trajet en cours s'efface devant lui. Notre propre ouverture passe.
    bloc.addEventListener("toggle", () => {
      if (trajet && bloc.open) return;
      trajet?.cancel();
      trajet = null;
      bloc.classList.remove("deroule", "deroule--ferme");
      ouvert = bloc.open;
    });
  }
}

// --- Six fenêtres, une seule -----------------------------------------------------
function reunis() {
  const section = document.querySelector("[data-reunis]");
  if (!section || calme) return;
  section.classList.add("reunis--pret");
  const jouer = () => {
    section.classList.remove("reunis--joue");
    void section.offsetWidth;
    section.classList.add("reunis--joue");
  };
  // La grande fenêtre sert de repère : sur un téléphone, la scène entière dépasse l'écran.
  vu(section.querySelector(".une"), jouer, 0.5);
  section.querySelector("[data-rejouer]")?.addEventListener("click", jouer);
}

// --- Affiche : le titre tient sur la largeur, le prix descend à zéro -----------
function affiche() {
  const titre = document.querySelector("[data-ajuste]");
  if (titre) {
    const ajuster = () => {
      titre.style.fontSize = "";
      const place = titre.parentElement.clientWidth;
      if (titre.scrollWidth > place) {
        titre.style.fontSize = `${(parseFloat(getComputedStyle(titre).fontSize) * place) / titre.scrollWidth}px`;
      }
    };
    ajuster();
    addEventListener("resize", ajuster);
    document.fonts?.ready.then(ajuster);
  }
  const prix = document.querySelector("[data-compte]");
  if (prix && !calme) {
    const fin = prix.textContent;
    const depart = Number(prix.dataset.compte);
    vu(
      prix,
      () => {
        const t0 = performance.now();
        const pas = (t) => {
          const p = borne((t - t0) / 1400, 0, 1);
          prix.textContent = fin.replace(
            /\d+/,
            String(Math.round(depart * (1 - p) ** 3)),
          );
          if (p < 1) requestAnimationFrame(pas);
        };
        requestAnimationFrame(pas);
      },
      0.9,
    );
  }
}

// --- Version publiée et boutons de téléchargement --------------------------------
async function version() {
  const boutons = [...document.querySelectorAll("[data-telecharger]")];
  const fichiers = [...document.querySelectorAll("[data-fichier]")];
  if (!boutons.length && !fichiers.length) return;
  const agent = navigator.userAgent;
  const systeme = /Windows/i.test(agent)
    ? "windows"
    : /Linux|X11/i.test(agent) && !/Android/i.test(agent)
      ? "linux"
      : null;
  document
    .querySelector(`.paquet[data-os="${systeme}"]`)
    ?.classList.add("paquet--ici");
  const libelle = (el, texte) => {
    if (texte) el.querySelector("span").textContent = texte;
  };

  let etat = null;
  try {
    const reponse = await fetch("/api/release", {
      headers: { Accept: "application/json" },
    });
    if (reponse.ok) etat = await reponse.json();
  } catch {
    // Service injoignable : les liens de la page restent tels quels.
  }
  if (!etat) return;

  if (!etat.published) {
    for (const el of [...boutons, ...fichiers])
      libelle(el, el.dataset.labelSoon);
    for (const el of fichiers) {
      el.removeAttribute("href");
      el.setAttribute("aria-disabled", "true");
    }
    document
      .querySelectorAll('[data-si="publie"]')
      .forEach((el) => (el.hidden = true));
    document
      .querySelectorAll('[data-si="bientot"]')
      .forEach((el) => (el.hidden = false));
    return;
  }
  document
    .querySelectorAll("[data-numero]")
    .forEach((el) => (el.textContent = etat.version));
  document
    .querySelectorAll("[data-version-ligne]")
    .forEach((el) => (el.hidden = false));
  document
    .querySelectorAll("[data-source]")
    .forEach((el) => (el.hidden = false));
  const unite = document.querySelector("[data-unite]")?.dataset.unite ?? "MB";
  for (const [nom, fichier] of Object.entries(etat.files ?? {})) {
    const taille = document.querySelector(`[data-taille="${nom}"]`);
    if (!fichier || !taille) continue;
    const mega = new Intl.NumberFormat(html.lang).format(
      Math.round(fichier.size / 1048576),
    );
    taille.textContent = `${taille.textContent} · ${mega}\u00a0${unite}`;
  }
  for (const bouton of boutons) {
    if (systeme === "windows") {
      libelle(bouton, bouton.dataset.labelWindows);
      bouton.setAttribute("href", "/download/windows");
    } else if (systeme === "linux") {
      libelle(bouton, bouton.dataset.labelLinux);
    }
  }
}

// --- Contact : fenêtre et envoi ---------------------------------------------------
function contact() {
  const dialogue = document.querySelector("[data-dialogue]");
  if (dialogue?.showModal) {
    document.querySelectorAll("[data-contact]").forEach((lien) =>
      lien.addEventListener("click", (e) => {
        e.preventDefault();
        dialogue.showModal();
      }),
    );
    dialogue
      .querySelector("[data-fermer]")
      ?.addEventListener("click", () => dialogue.close());
    // Un clic sur le fond (hors du cadre) ferme la fenêtre ; pas un glisser
    // commencé dans le formulaire (sélection de texte) et relâché sur le fond.
    let surLeFond = false;
    dialogue.addEventListener("pointerdown", (e) => {
      surLeFond = e.target === dialogue;
    });
    dialogue.addEventListener("click", (e) => {
      if (e.target === dialogue && surLeFond) dialogue.close();
    });
  }

  for (const formulaire of document.querySelectorAll("[data-formulaire]")) {
    const etat = formulaire.querySelector("[data-etat]");
    const bouton = formulaire.querySelector("button[type=submit]");
    const messages = formulaire.dataset;
    // Les messages d'erreur sont ceux de la page, dans sa langue.
    formulaire.noValidate = true;
    // Temps passé sur la page avant l'envoi : un robot poste aussitôt, pas un lecteur.
    const debut = performance.now();
    const dire = (code, champ) => {
      etat.textContent =
        messages[`msg${code[0].toUpperCase()}${code.slice(1)}`] ??
        messages.msgSend;
      etat.className = "etat etat--erreur";
      for (const el of formulaire.elements) {
        el.removeAttribute?.("aria-invalid");
        el.removeAttribute?.("aria-describedby");
      }
      if (champ) {
        // Le champ fautif porte la raison : elle est lue avec lui.
        champ.setAttribute("aria-invalid", "true");
        champ.setAttribute("aria-describedby", etat.id);
        champ.focus();
      } else {
        etat.tabIndex = -1;
        etat.focus();
      }
    };
    formulaire.addEventListener("submit", async (e) => {
      e.preventDefault();
      const {
        name,
        email,
        message,
        champ_libre: piege,
        lang,
      } = formulaire.elements;
      if (!name.value.trim()) return dire("name", name);
      if (!email.validity.valid || !email.value.trim())
        return dire("email", email);
      if (message.value.trim().length < 10) return dire("message", message);
      bouton.disabled = true;
      const libelle = bouton.textContent;
      bouton.textContent = messages.msgSending;
      let code;
      try {
        const reponse = await fetch(formulaire.action, {
          method: "POST",
          headers: {
            "Content-Type": "application/json",
            Accept: "application/json",
          },
          body: JSON.stringify({
            name: name.value,
            email: email.value,
            message: message.value,
            champ_libre: piege.value,
            lang: lang.value,
            elapsed: Math.round(performance.now() - debut),
          }),
        });
        const resultat = await reponse.json().catch(() => ({}));
        code = resultat.ok ? "" : (resultat.error ?? "send");
      } catch {
        code = "send";
      }
      bouton.disabled = false;
      bouton.textContent = libelle;
      if (code) return dire(code, formulaire.elements[code] ?? null);
      etat.textContent = "";
      formulaire.classList.add("formulaire--parti");
      const merci = formulaire.querySelector("[data-merci]");
      merci.hidden = false;
      merci.setAttribute("tabindex", "-1");
      merci.focus();
    });
  }
}

for (const partie of [
  entete,
  chatons,
  fil,
  maisonFixe,
  demo,
  questions,
  reunis,
  affiche,
  version,
  contact,
]) {
  try {
    partie();
  } catch (erreur) {
    // Sans le fil, les raisons doivent rester lisibles (elles l'attendent pour paraître).
    if (partie === fil) html.classList.add("sans-fil");
    console.error(erreur);
  }
}
