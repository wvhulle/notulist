let
  whisperModelTable = builtins.fromTOML (builtins.readFile ../notulist-server/whisper-models.toml);
in
{
  whisperModelNames = builtins.attrNames whisperModelTable.models;

  defaultWhisperModelForAcceleration = {
    cuda = "large-v3-turbo";
    vulkan = "large-v3-turbo";
    cpu = "base";
  };

  fetchWhisperModel =
    fetchurl: modelName:
    let
      chosenModel = whisperModelTable.models.${modelName};
    in
    fetchurl {
      name = chosenModel.file_name;
      url = "${whisperModelTable.download_base_url}/${chosenModel.file_name}";
      inherit (chosenModel) sha256;
    };
}
