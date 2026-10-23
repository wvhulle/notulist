let
  huggingFaceModel =
    fileName: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/${fileName}";
in
{
  "tiny" = {
    url = huggingFaceModel "ggml-tiny.bin";
    hash = "sha256-vgfgSOHlma1GNByNKhNWRQl6U4IhZ4t6zdGxkZxuGyE=";
  };
  "tiny.en" = {
    url = huggingFaceModel "ggml-tiny.en.bin";
    hash = "sha256-kh5M+Ghv3Zk9zQgaXaW2w2W/3hFi5ysI11rHUomSCx8=";
  };
  "base" = {
    url = huggingFaceModel "ggml-base.bin";
    hash = "sha256-YO1bw90U7qhWST0zQ0m0BXgt3K8AKNS130CINF+6Lv4=";
  };
  "base.en" = {
    url = huggingFaceModel "ggml-base.en.bin";
    hash = "sha256-oDd5yG3zMjB19eeWyyzlAp8A7Ihp7uP9+4l6/jbG0AI=";
  };
  "small" = {
    url = huggingFaceModel "ggml-small.bin";
    hash = "sha256-G+OpsgY4Z7k35k4ux0gzZKeZF+FX+pjF2UtcH//qmHs=";
  };
  "small.en" = {
    url = huggingFaceModel "ggml-small.en.bin";
    hash = "sha256-xhONbVjsyDIgl+D5h8MvG+i7ChhTKj+I9zTRu/nEHl0=";
  };
  "medium" = {
    url = huggingFaceModel "ggml-medium.bin";
    hash = "sha256-bBTVre5fhjlAN7Tk6LWfFnO2zuEOPPCxG72+55wVYgg=";
  };
  "medium.en" = {
    url = huggingFaceModel "ggml-medium.en.bin";
    hash = "sha256-zDfpNHgzjsdwAoGnrDChASiSnrj0J92i6GX6qPbaQ1Y=";
  };
  "large-v3" = {
    url = huggingFaceModel "ggml-large-v3.bin";
    hash = "sha256-ZNGCtEC5jVIDxPm9VBVE2ExgUZbE97hF36EfsjWU0eI=";
  };
  "large-v3-turbo" = {
    url = huggingFaceModel "ggml-large-v3-turbo.bin";
    hash = "sha256-H8cPd0046xaZk6w5Huo1fvR8iHV+9y7llDh5t+jivGk=";
  };
}
