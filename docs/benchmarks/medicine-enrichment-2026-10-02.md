# Enriquecimento de medicamentos — 02/10/2026

Execução final reutilizando as oito fontes preservadas, sem downloads novos. Lotes em `runs/sncr-2026-10-02/work/`, mantendo separação `ativo/` e `inativos/`.

## Cobertura

- 43,528 medicamentos; 4,573 com ao menos um dos sete tipos SNCR.
- 61,533 apresentações; concentração extraída em 55,054 (89.5%).
- Forma física extraída em 50,971 (82.8%); ambos os campos em 49,112.

| Tipo | Medicamentos |
| --- | ---: |
| NRA | 1.463 |
| NRB | 526 |
| NRB2 | 12 |
| NRR | 29 |
| NRT | 5 |
| RCE | 1.614 |
| RET | 981 |

Um medicamento pode acumular tipos quando a fonte contém mais de uma restrição específica. Os totais por tipo não são uma contagem de produtos distintos. `[]` não significa isenção de prescrição. Adendos dependentes de dose/formulação não são calculados; concentração e forma só são extraídas com os padrões reconhecidos. As descrições anteriores permanecem disponíveis. [Regras e fontes oficiais do SNCR](../research/sncr-prescription-types.md).

## Validação e desempenho

- Todos os campos anteriores dos 43.528 medicamentos foram comparados por JSON canônico, retirando apenas os três novos campos, e permaneceram idênticos.
- Contagens, SHA-256 das fontes e conteúdo canônico de cosméticos, cannabis, Portaria 344 e SIGTAP permaneceram idênticos à execução anterior.
- Os 19 testes, `cargo fmt --check`, `cargo check --locked` e `git diff --check` passaram. As ferramentas Rust foram executadas via mise.
- Execução final: 66.399 s; pico RSS 586.32 MiB. Uma medição com os dados enriquecidos; não constitui uma comparação estatística de desempenho.

Métricas completas em `runs/sncr-2026-10-02/validated.json`; cobertura em `runs/sncr-2026-10-02/coverage.json`.
