# Catálogo Saúde

CLI Rust para baixar fontes de catálogos e produzir lotes JSON normalizados para importação posterior pelo app `catalog`. Não grava no banco de dados.

## Requisitos

O projeto fixa Rust via mise em `mise.toml`. Instale/ative mise e execute os comandos abaixo na pasta do projeto:

```sh
mise install
mise exec -- cargo run -- --output ./runs/current --batch-size 1000
```

Para retomar uma execução usando arquivos já baixados e validados pelo SHA-1, acrescente `--reuse-sources`.

A execução padrão coleta medicamentos, cosméticos, cannabis, Portaria 344 e SIGTAP/TUSS/CID-10. DCB está fora do escopo. Referências existentes de `_dumps/anvisa` ficam em `references/anvisa` e são lidas localmente.

Cada execução grava os arquivos brutos em `sources/` e mantém `download-control.json` com nome, data do download e SHA-1 de cada cópia. O `manifest.json` registra as URLs, hashes SHA-256, contagens e tempos por etapa. Os lotes JSON são nomeados sequencialmente e escritos por arquivo temporário.

A expansão das descrições de apresentações usa o vocabulário controlado de formas farmacêuticas, vias de administração e embalagens da [Anvisa](https://www.gov.br/anvisa/pt-br/centraisdeconteudo/publicacoes/medicamentos/publicacoes-sobre-medicamentos/vocabulario-controlado.pdf/@@display-file/file). `references/presentation_abbreviations.json` contém as expressões compostas e seus componentes presentes nesse vocabulário. Abreviações sem correspondência oficial permanecem inalteradas.

O código fica dividido por responsabilidade: `main.rs` orquestra a CLI; `downloads.rs` obtém e registra as fontes; `anvisa.rs` normaliza os catálogos da Anvisa; `sigtap.rs` lê SIGTAP/TUSS/CID-10; `output.rs` grava os lotes.
