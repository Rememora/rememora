/* ============================================
   Rememora — Neural Network Canvas + Interactions
   ============================================ */

// --- Neural Network Particle Animation ---
class NeuralCanvas {
  constructor(canvas) {
    this.canvas = canvas;
    this.ctx = canvas.getContext('2d');
    this.particles = [];
    this.connections = [];
    this.mouse = { x: -1000, y: -1000 };
    this.dpr = window.devicePixelRatio || 1;
    this.particleCount = window.innerWidth < 768 ? 40 : 80;
    this.connectionDistance = window.innerWidth < 768 ? 120 : 180;
    this.running = true;

    this.resize();
    this.init();
    this.bindEvents();
    this.animate();
  }

  resize() {
    const rect = this.canvas.parentElement.getBoundingClientRect();
    this.width = rect.width;
    this.height = rect.height;
    this.canvas.width = this.width * this.dpr;
    this.canvas.height = this.height * this.dpr;
    this.canvas.style.width = this.width + 'px';
    this.canvas.style.height = this.height + 'px';
    this.ctx.scale(this.dpr, this.dpr);
  }

  init() {
    this.particles = [];
    for (let i = 0; i < this.particleCount; i++) {
      this.particles.push({
        x: Math.random() * this.width,
        y: Math.random() * this.height,
        vx: (Math.random() - 0.5) * 0.5,
        vy: (Math.random() - 0.5) * 0.5,
        radius: Math.random() * 2 + 1,
        opacity: Math.random() * 0.5 + 0.2,
        hue: Math.random() > 0.5 ? 258 : 174, // purple or teal
        pulse: Math.random() * Math.PI * 2,
      });
    }
  }

  bindEvents() {
    window.addEventListener('resize', () => {
      this.resize();
      this.particleCount = window.innerWidth < 768 ? 40 : 80;
      this.connectionDistance = window.innerWidth < 768 ? 120 : 180;
    });

    this.canvas.addEventListener('mousemove', (e) => {
      const rect = this.canvas.getBoundingClientRect();
      this.mouse.x = e.clientX - rect.left;
      this.mouse.y = e.clientY - rect.top;
    });

    this.canvas.addEventListener('mouseleave', () => {
      this.mouse.x = -1000;
      this.mouse.y = -1000;
    });

    // Pause when not visible
    const observer = new IntersectionObserver((entries) => {
      this.running = entries[0].isIntersecting;
    });
    observer.observe(this.canvas);
  }

  animate() {
    if (!this.running) {
      requestAnimationFrame(() => this.animate());
      return;
    }

    this.ctx.clearRect(0, 0, this.width, this.height);

    // Update particles
    for (const p of this.particles) {
      p.x += p.vx;
      p.y += p.vy;
      p.pulse += 0.02;

      // Bounce off edges
      if (p.x < 0 || p.x > this.width) p.vx *= -1;
      if (p.y < 0 || p.y > this.height) p.vy *= -1;

      // Mouse interaction
      const dx = this.mouse.x - p.x;
      const dy = this.mouse.y - p.y;
      const dist = Math.sqrt(dx * dx + dy * dy);
      if (dist < 200) {
        const force = (200 - dist) / 200 * 0.02;
        p.vx += dx * force;
        p.vy += dy * force;
      }

      // Damping
      p.vx *= 0.99;
      p.vy *= 0.99;
    }

    // Draw connections
    for (let i = 0; i < this.particles.length; i++) {
      for (let j = i + 1; j < this.particles.length; j++) {
        const a = this.particles[i];
        const b = this.particles[j];
        const dx = a.x - b.x;
        const dy = a.y - b.y;
        const dist = Math.sqrt(dx * dx + dy * dy);

        if (dist < this.connectionDistance) {
          const opacity = (1 - dist / this.connectionDistance) * 0.15;
          const gradient = this.ctx.createLinearGradient(a.x, a.y, b.x, b.y);
          gradient.addColorStop(0, `hsla(${a.hue}, 80%, 65%, ${opacity})`);
          gradient.addColorStop(1, `hsla(${b.hue}, 80%, 65%, ${opacity})`);

          this.ctx.beginPath();
          this.ctx.moveTo(a.x, a.y);
          this.ctx.lineTo(b.x, b.y);
          this.ctx.strokeStyle = gradient;
          this.ctx.lineWidth = 1;
          this.ctx.stroke();
        }
      }
    }

    // Draw particles
    for (const p of this.particles) {
      const pulseRadius = p.radius + Math.sin(p.pulse) * 0.5;
      const pulseOpacity = p.opacity + Math.sin(p.pulse) * 0.1;

      // Glow
      this.ctx.beginPath();
      this.ctx.arc(p.x, p.y, pulseRadius * 4, 0, Math.PI * 2);
      this.ctx.fillStyle = `hsla(${p.hue}, 80%, 65%, ${pulseOpacity * 0.1})`;
      this.ctx.fill();

      // Core
      this.ctx.beginPath();
      this.ctx.arc(p.x, p.y, pulseRadius, 0, Math.PI * 2);
      this.ctx.fillStyle = `hsla(${p.hue}, 80%, 65%, ${pulseOpacity})`;
      this.ctx.fill();
    }

    requestAnimationFrame(() => this.animate());
  }
}

// --- Scroll Reveal ---
function initReveal() {
  const reveals = document.querySelectorAll('.reveal');
  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          entry.target.classList.add('visible');
        }
      });
    },
    { threshold: 0.1, rootMargin: '0px 0px -50px 0px' }
  );

  reveals.forEach((el) => observer.observe(el));
}

// --- Sticky Nav ---
function initNav() {
  const nav = document.querySelector('.nav');
  if (!nav) return;

  window.addEventListener('scroll', () => {
    nav.classList.toggle('scrolled', window.scrollY > 50);
  });

  // Mobile toggle
  const toggle = document.querySelector('.nav-toggle');
  const links = document.querySelector('.nav-links');
  if (toggle && links) {
    toggle.addEventListener('click', () => {
      links.classList.toggle('open');
      toggle.classList.toggle('active');
    });

    // Close on link click
    links.querySelectorAll('a').forEach((a) => {
      a.addEventListener('click', () => {
        links.classList.remove('open');
        toggle.classList.remove('active');
      });
    });
  }
}

// --- Copy Buttons ---
function initCopyButtons() {
  document.querySelectorAll('.code-copy').forEach((btn) => {
    btn.addEventListener('click', () => {
      const codeBlock = btn.closest('.code-window').querySelector('pre');
      const text = codeBlock.textContent;
      navigator.clipboard.writeText(text).then(() => {
        const orig = btn.textContent;
        btn.textContent = 'Copied!';
        btn.style.color = '#55efc4';
        setTimeout(() => {
          btn.textContent = orig;
          btn.style.color = '';
        }, 2000);
      });
    });
  });

  // Install command copy
  document.querySelectorAll('.install-command').forEach((el) => {
    el.addEventListener('click', () => {
      const text = el.querySelector('.text').textContent;
      navigator.clipboard.writeText(text).then(() => {
        const icon = el.querySelector('.copy-icon');
        if (icon) {
          const orig = icon.textContent;
          icon.textContent = 'Copied!';
          icon.style.color = '#55efc4';
          setTimeout(() => {
            icon.textContent = orig;
            icon.style.color = '';
          }, 2000);
        }
      });
    });
  });
}

// --- Typing Effect for Hero Terminal ---
function initTypingEffect() {
  const typingEl = document.querySelector('.typing-effect');
  if (!typingEl) return;

  const lines = typingEl.getAttribute('data-lines');
  if (!lines) return;

  const lineArr = JSON.parse(lines);
  let lineIdx = 0;
  let charIdx = 0;
  let currentText = '';
  let deleting = false;
  let pauseTimer = 0;

  function type() {
    if (lineIdx >= lineArr.length) lineIdx = 0;
    const currentLine = lineArr[lineIdx];

    if (!deleting) {
      currentText = currentLine.substring(0, charIdx + 1);
      charIdx++;

      if (charIdx === currentLine.length) {
        pauseTimer = 2000;
        deleting = true;
      }
    } else {
      if (pauseTimer > 0) {
        pauseTimer -= 50;
        setTimeout(type, 50);
        typingEl.textContent = currentText;
        return;
      }

      currentText = currentLine.substring(0, charIdx - 1);
      charIdx--;

      if (charIdx === 0) {
        deleting = false;
        lineIdx++;
      }
    }

    typingEl.textContent = currentText;
    const speed = deleting ? 30 : 60;
    setTimeout(type, speed);
  }

  type();
}

// --- Counter Animation ---
function initCounters() {
  const counters = document.querySelectorAll('.stat-value[data-count]');
  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          const el = entry.target;
          const target = el.getAttribute('data-count');
          const suffix = el.getAttribute('data-suffix') || '';
          const prefix = el.getAttribute('data-prefix') || '';
          const isFloat = target.includes('.');
          const targetNum = parseFloat(target);
          const duration = 1500;
          const startTime = performance.now();

          function updateCount(currentTime) {
            const elapsed = currentTime - startTime;
            const progress = Math.min(elapsed / duration, 1);
            const eased = 1 - Math.pow(1 - progress, 3);
            const current = isFloat
              ? (targetNum * eased).toFixed(1)
              : Math.floor(targetNum * eased);
            el.textContent = prefix + current + suffix;

            if (progress < 1) {
              requestAnimationFrame(updateCount);
            } else {
              el.textContent = prefix + target + suffix;
            }
          }

          requestAnimationFrame(updateCount);
          observer.unobserve(el);
        }
      });
    },
    { threshold: 0.5 }
  );

  counters.forEach((el) => observer.observe(el));
}

// --- Docs Sidebar Active State ---
function initDocsSidebar() {
  const headings = document.querySelectorAll('.docs-content h2[id], .docs-content h3[id]');
  const links = document.querySelectorAll('.docs-sidebar-links a');

  if (headings.length === 0) return;

  const observer = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          links.forEach((l) => l.classList.remove('active'));
          const activeLink = document.querySelector(
            `.docs-sidebar-links a[href="#${entry.target.id}"]`
          );
          if (activeLink) activeLink.classList.add('active');
        }
      });
    },
    { rootMargin: '-80px 0px -60% 0px', threshold: 0 }
  );

  headings.forEach((h) => observer.observe(h));

  // Mobile sidebar toggle
  const toggle = document.querySelector('.docs-sidebar-toggle');
  const sidebar = document.querySelector('.docs-sidebar');
  if (toggle && sidebar) {
    toggle.addEventListener('click', () => {
      sidebar.classList.toggle('open');
    });
  }
}

// --- Smooth Scroll for Anchor Links ---
function initSmoothScroll() {
  document.querySelectorAll('a[href^="#"]').forEach((anchor) => {
    anchor.addEventListener('click', function (e) {
      const target = document.querySelector(this.getAttribute('href'));
      if (target) {
        e.preventDefault();
        target.scrollIntoView({ behavior: 'smooth', block: 'start' });
        // Update URL without scrolling
        history.pushState(null, null, this.getAttribute('href'));
      }
    });
  });
}

// --- Init ---
document.addEventListener('DOMContentLoaded', () => {
  // Neural canvas
  const canvas = document.getElementById('neural-canvas');
  if (canvas) {
    new NeuralCanvas(canvas);
  }

  initNav();
  initReveal();
  initCopyButtons();
  initTypingEffect();
  initCounters();
  initDocsSidebar();
  initSmoothScroll();
});
