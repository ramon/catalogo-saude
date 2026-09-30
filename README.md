# Catálogo Saúde

O Catálogo Saúde reúne dados públicos de medicamentos e outros catálogos de saúde em arquivos JSON prontos para integração. Ele baixa as fontes, cruza os registros relacionados e normaliza os dados; não se conecta nem grava dados diretamente em um banco.

## Catálogos

- **Medicamentos:** combina dados abertos, consulta de produtos e preços da Anvisa.
- **Cosméticos e produtos de cannabis:** consulta de produtos da Anvisa.
- **Portaria 344:** listas de substâncias sujeitas a controle especial.
- **SIGTAP, TUSS e CID-10:** procedimentos, vínculos com a TUSS e códigos CID-10.

O catálogo DCB não faz parte desta coleta. Arquivos regulatórios já mantidos em `references/anvisa/` são usados localmente.

## Executar

É necessário ter o [mise](https://mise.jdx.dev/) instalado. O projeto fixa a versão do Rust em `mise.toml`; instale-a e execute uma coleta em um diretório próprio:

```sh
mise install
mise exec -- cargo run --release -- --output ./runs/minha-coleta
```

O comando exibe o andamento de cada fonte e etapa. Para escolher outro tamanho de lote, use `--batch-size` (o padrão é `1000`).

Também é possível baixar o executável da [versão v0.1.0](https://github.com/ramon/catalogo-saude/releases/tag/v0.1.0), atualmente publicada para Linux x86_64 com GNU libc. Depois de extrair o pacote:

```sh
./catalogo-saude-release-v0.1.0/catalogo-saude --output ./runs/minha-coleta
```

O executável recebe as mesmas opções da CLI, por exemplo `--output` e `--batch-size`.

## Arquivos gerados

Cada coleta cria um diretório com os lotes JSON por catálogo, os arquivos brutos usados e dois arquivos de controle:

- `manifest.json`: fontes e URLs, SHA-256, quantidade de registros e tempo de cada etapa.
- `download-control.json`: nome e data de cada download, caminho do arquivo e SHA-1 usado para validar sua integridade.
- `sources/`: cópias dos arquivos baixados, preservadas para consulta e reprocessamento.

Os lotes ficam em pastas como `medicines/`, `cosmetics/`, `cannabis/`, `portaria-344/lists/` e `sigtap/`. Os arquivos são numerados, por exemplo `batch-000001.json`, com até mil registros cada por padrão.

Guarde o diretório da coleta se quiser manter as fontes usadas. Para normalizar novamente usando esses downloads, informe o mesmo diretório e `--reuse-sources`:

```sh
mise exec -- cargo run --release -- --output ./runs/minha-coleta --reuse-sources
```

O loader valida o SHA-1 antes de reutilizar cada fonte. Se um arquivo preservado tiver sido alterado, a execução para em vez de usá-lo silenciosamente.

## Descrições de apresentações

As descrições de apresentações usam o [Vocabulário Controlado da Anvisa](https://www.gov.br/anvisa/pt-br/centraisdeconteudo/publicacoes/medicamentos/publicacoes-sobre-medicamentos/vocabulario-controlado.pdf/@@display-file/file) para expandir formas farmacêuticas, vias de administração e embalagens. Siglas sem correspondência nesse vocabulário permanecem como vieram da fonte.

## Fontes

- [Dados abertos de medicamentos da Anvisa](https://dados.anvisa.gov.br/dados/DADOS_ABERTOS_MEDICAMENTOS.csv)
- [Consultas de produtos da Anvisa](https://dados.anvisa.gov.br/dados/CONSULTAS/PRODUTOS/)
- [Portaria SVS/MS nº 344/1998](https://anvisalegis.datalegis.net/)
- [Tabelas SIGTAP](https://github.com/RenatoKR/SIGTAP)
- [Tabelas TISS da ANS](https://www.gov.br/ans/pt-br/assuntos/prestadores/padrao-para-troca-de-informacao-de-saude-suplementar-tiss)
