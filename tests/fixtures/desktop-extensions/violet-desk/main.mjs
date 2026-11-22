export default function mount(api) {
  const b = document.createElement('button');
  b.textContent = 'Read snapshot';
  b.onclick = async () => { b.textContent = String((await api.request('snapshot', {})).instances.length); };
  document.body.append(b);
}
