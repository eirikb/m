cd .cache/m || exit
for gg in gg*
do
  chmod +x "$gg"
  # shellcheck disable=SC2086
  if "./$gg"; then
    ./mn -- "$USER_PWD" $1
    exit
  fi
done

echo "Failed?!"
exit 1