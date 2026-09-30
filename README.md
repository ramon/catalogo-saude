# Catalog Data Loader

CLI Rust para baixar fontes de catálogos e produzir lotes JSON normalizados para importação posterior pelo app `catalog`. Não grava no banco de dados.

## Requisitos

O projeto fixa Rust via mise em `mise.toml`. Instale/ative mise e execute os comandos abaixo na pasta do projeto:

```sh
mise install
mise exec -- cargo run -- --output ./runs/current --batch-size 1000
```

A execução padrão coleta medicamentos, cosméticos, cannabis, Portaria 344 e SIGTAP/TUSS/CID-10. DCB está fora do escopo. Referências existentes de `_dumps/anvisa` ficam em `references/anvisa` e são lidas localmente.

Cada execução grava arquivos JSON em diretório de saída, com manifesto de fontes, hashes SHA-256, contagens e lotes nomeados sequencialmente. Os lotes são escritos em arquivos temporários e renomeados ao concluir cada arquivo.
